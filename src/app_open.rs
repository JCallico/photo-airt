//! Opening photos from anywhere (files, links, clipboard, recents) and the
//! session's photos. Every photo keeps its own studio state — collection,
//! art direction, style previews — parked while another photo is on stage,
//! and AI jobs deliver to the photo they were started for.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{ColorImage, TextureHandle, TextureOptions, Vec2};

use crate::ai::{AiOutput, Job};
use crate::app::{App, Artwork, Director, Kind, LoadState, Photo, Session, THUMB_LONG, TrayItem, WorkerMsg, color_image};
use crate::finish::Finish;
use crate::imaging::Img;
use crate::photo_io;
use crate::sources::{self, Asset, Clip, Input, Recents};

/// Something the user asked to open.
#[derive(Clone, Debug)]
pub enum OpenRequest {
    Input(Input),
    /// A photo we already hold locally (session photos, recents) — keeps its origin.
    Asset(Asset),
    ClipboardImage,
}

/// What the clipboard holds, checked when the Open sheet appears.
#[derive(Default)]
pub enum ClipStatus {
    #[default]
    Unknown,
    Image {
        width: usize,
        height: usize,
        preview: ColorImage,
    },
    Text(String),
    Empty,
    Unavailable(String),
}

#[derive(Default)]
pub struct OpenSheet {
    pub visible: bool,
    pub input: String,
    pub focus: bool,
    pub clip: ClipStatus,
    pub clip_tex: Option<TextureHandle>,
    /// The clipboard link was offered in the smart bar already.
    pub prefilled: bool,
}

/// The "All photos" contact sheet.
#[derive(Default)]
pub struct Overview {
    pub visible: bool,
    pub filter: String,
}

const RECENT_THUMB: u32 = 360;

impl App {
    // ------------------------------------------------------------ entry points

    pub fn show_open_sheet(&mut self) {
        self.open_sheet.visible = true;
        self.open_sheet.focus = true;
        self.open_sheet.clip = ClipStatus::Unknown;
        self.open_sheet.clip_tex = None;
        self.open_sheet.prefilled = false;
        self.check_clipboard();
    }

    fn check_clipboard(&self) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let status = match sources::read_clipboard() {
                Ok(Clip::Image { width, height, rgba }) => {
                    let preview = image::RgbaImage::from_raw(width as u32, height as u32, rgba)
                        .map(|img| image::imageops::thumbnail(&img, 320, (320 * height / width.max(1)).max(1) as u32))
                        .map(|t| ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw()));
                    match preview {
                        Some(preview) => ClipStatus::Image { width, height, preview },
                        None => ClipStatus::Empty,
                    }
                }
                Ok(Clip::Text(t)) => ClipStatus::Text(t),
                Ok(Clip::Empty) => ClipStatus::Empty,
                Err(e) => ClipStatus::Unavailable(format!("{e:#}")),
            };
            let _ = tx.send(WorkerMsg::Clipboard(status));
            ctx.request_repaint();
        });
    }

    pub fn pick_files(&mut self) {
        if self.picking {
            return;
        }
        self.picking = true;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let paths = rfd::FileDialog::new()
                .set_title("Open photos")
                .add_filter("Images", &["jpg", "jpeg", "png", "webp", "tif", "tiff", "bmp", "gif", "heic", "heif", "avif"])
                .pick_files()
                .unwrap_or_default();
            let _ = tx.send(WorkerMsg::FilesPicked(paths));
            ctx.request_repaint();
        });
    }

    /// Text pasted with Ctrl+V: one or several links/paths, one per line.
    pub fn open_text(&mut self, text: &str) {
        let inputs = sources::classify_all(text);
        if inputs.is_empty() {
            let e = sources::classify(text).err().unwrap_or_default();
            self.toast(e, crate::theme::MUTED);
            return;
        }
        self.open(inputs.into_iter().map(OpenRequest::Input).collect());
    }

    pub fn open_recent(&mut self, idx: usize) {
        let Some(r) = self.recents.items.get(idx).cloned() else { return };
        if r.asset.local.is_file() {
            self.open(vec![OpenRequest::Asset(r.asset)]);
            return;
        }
        match Recents::reopen_input(&r) {
            Ok(input) => self.open(vec![OpenRequest::Input(input)]),
            Err(e) => {
                self.toast(e, crate::theme::DANGER);
                self.recents.remove(&r.asset.local);
                self.recents.save();
            }
        }
    }

    /// Open one or more photos. The first is put on stage; the rest join
    /// the collection as stacks. Runs off the UI thread with live progress.
    pub fn open(&mut self, reqs: Vec<OpenRequest>) {
        if reqs.is_empty() {
            return;
        }
        if let Some(l) = &self.loading {
            l.cancel.store(true, Ordering::Relaxed);
        }
        let batch = self.next_id;
        self.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let label = match &reqs[0] {
            OpenRequest::Input(Input::File(p)) => p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
            OpenRequest::Input(Input::Link(u)) => u.host_str().unwrap_or("the web").to_string(),
            OpenRequest::Asset(a) => a.name.clone(),
            OpenRequest::ClipboardImage => "the clipboard".into(),
        };
        let label = if reqs.len() > 1 { format!("{label} + {} more", reqs.len() - 1) } else { label };
        self.loading = Some(LoadState { batch, label, stage: "Opening…".into(), done: 0, total: None, cancel: cancel.clone() });
        self.open_sheet.visible = false;

        let (tx, ctx, res) = (self.tx.clone(), self.ctx.clone(), self.work_res);
        std::thread::spawn(move || {
            let send = |m: WorkerMsg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            let mut staged = false;
            let total = reqs.len();
            for (i, req) in reqs.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let mut report = |p: sources::Progress| send(WorkerMsg::Acquiring { batch, progress: p });
                let asset = match req {
                    OpenRequest::Input(input) => sources::acquire(&input, &mut report, &cancel),
                    OpenRequest::Asset(a) if a.local.is_file() => Ok(a),
                    OpenRequest::Asset(a) => Err(anyhow::anyhow!("{} is no longer available", a.name)),
                    OpenRequest::ClipboardImage => match sources::read_clipboard() {
                        Ok(Clip::Image { width, height, rgba }) => sources::clipboard_asset(width, height, &rgba, &sources::sources_dir()),
                        Ok(_) => Err(anyhow::anyhow!("The clipboard has no image")),
                        Err(e) => Err(e),
                    },
                };
                let asset = match asset {
                    Ok(a) => a,
                    Err(e) => {
                        send(WorkerMsg::AcquireFailed { error: format!("{e:#}") });
                        continue;
                    }
                };
                if total > 1 {
                    report(sources::Progress { stage: format!("Developing {} of {total}…", i + 1), done: 0, total: None });
                } else {
                    report(sources::Progress { stage: "Developing…".into(), done: 0, total: None });
                }
                let rgb = match photo_io::load_photo(&asset.local) {
                    Ok(rgb) => rgb,
                    Err(e) => {
                        send(WorkerMsg::AcquireFailed { error: format!("Could not open {}: {e:#}", asset.name) });
                        continue;
                    }
                };
                // Stack / recents thumbnail.
                let (w, h) = (rgb.width(), rgb.height());
                let s = RECENT_THUMB as f32 / w.max(h) as f32;
                let small = image::imageops::thumbnail(&rgb, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1));
                let thumb_file = Recents::thumb_path(&asset);
                if let Some(dir) = thumb_file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = small.save(&thumb_file);
                let thumb = ColorImage::from_rgb([small.width() as usize, small.height() as usize], small.as_raw());
                send(WorkerMsg::Acquired { asset: asset.clone(), thumb, thumb_file });
                if !staged {
                    staged = true;
                    let orig = [w, h];
                    let work = Img::from_rgb8(&rgb).fit_long(res);
                    let thumb = work.fit_long(THUMB_LONG);
                    let color = color_image(&work);
                    send(WorkerMsg::PhotoLoaded { batch, result: Ok((asset, work, thumb, color, orig)) });
                }
            }
            send(WorkerMsg::AcquireDone { batch });
        });
    }

    pub fn cancel_loading(&mut self) {
        if let Some(l) = self.loading.take() {
            l.cancel.store(true, Ordering::Relaxed);
            self.toast("Cancelled", crate::theme::MUTED);
        }
    }

    // ------------------------------------------------------------ arrivals

    pub(crate) fn on_acquired(&mut self, asset: Asset, thumb: ColorImage, thumb_file: PathBuf) {
        let tex = self.ctx.load_texture(format!("tray-{}", asset.local.display()), thumb, TextureOptions::LINEAR);
        match self.tray.iter_mut().find(|t| t.asset.local == asset.local) {
            Some(t) => {
                t.thumb = tex.clone();
                t.asset = asset.clone();
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                self.tray.push(TrayItem { id, asset: asset.clone(), thumb: tex.clone(), session: None });
            }
        }
        self.recent_tex.insert(thumb_file.clone(), tex);
        self.recents.push(asset, thumb_file);
    }

    pub(crate) fn on_photo(&mut self, asset: Asset, work: Img, thumb: Img, color: ColorImage, orig: [u32; 2]) {
        self.park_current();
        let tex = self.ctx.load_texture("photo", color, TextureOptions::LINEAR);
        let id = match self.tray.iter().find(|t| t.asset.local == asset.local) {
            Some(t) => t.id,
            None => {
                let id = self.next_id;
                self.next_id += 1;
                let small = color_image(&thumb.fit_long(RECENT_THUMB as usize));
                let thumb_tex = self.ctx.load_texture(format!("tray-{id}"), small, TextureOptions::LINEAR);
                self.tray.push(TrayItem { id, asset: asset.clone(), thumb: thumb_tex, session: None });
                id
            }
        };
        self.current = Some(id);
        self.switching_to = None;
        self.film_focus = true;
        self.photo = Some(Photo {
            path: asset.local.clone(),
            name: asset.name.clone(),
            asset,
            work: Arc::new(work),
            thumb: Arc::new(thumb),
            tex,
            orig_size: orig,
        });
        self.artworks.clear();
        self.selected = None;
        self.director = None;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.render_thumbs();
        self.render_due = Some(self.now());
    }

    // ------------------------------------------------------------ session photos

    /// Park the photo on stage (with its whole studio state) as a stack.
    fn park_current(&mut self) {
        if let Some(r) = self.render.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.render_due = None;
        self.reveal = None;
        self.thumb_gen += 1;
        let (Some(cur), Some(photo)) = (self.current, self.photo.take()) else { return };
        let session = Session {
            photo,
            artworks: std::mem::take(&mut self.artworks),
            selected: self.selected.take(),
            director: self.director.take(),
            thumbs: std::mem::take(&mut self.thumbs),
        };
        if let Some(t) = self.tray.iter_mut().find(|t| t.id == cur) {
            t.session = Some(session);
        }
    }

    /// Bring a session photo on stage — instantly if it was open before.
    pub fn switch_to(&mut self, id: u64) {
        if self.current == Some(id) {
            // Changed our mind mid-switch: stay on the photo already shown.
            if self.switching_to.take().is_some()
                && let Some(l) = self.loading.take()
            {
                l.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        let Some(idx) = self.tray.iter().position(|t| t.id == id) else { return };
        match self.tray[idx].session.take() {
            Some(session) => {
                self.park_current();
                self.current = Some(id);
                self.switching_to = None;
                self.film_focus = true;
                self.photo = Some(session.photo);
                self.artworks = session.artworks;
                self.selected = session.selected;
                self.director = session.director;
                self.thumbs = session.thumbs;
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
                if self.thumbs.len() < crate::plugins::registry().all().len() {
                    self.render_thumbs();
                }
                if self.director.as_ref().is_some_and(|d| d.thumbs.is_empty() && !d.report.recipes.is_empty()) {
                    self.render_recipe_thumbs();
                }
                if self.artworks.is_empty() {
                    self.render_due = Some(self.now());
                }
            }
            None => {
                let asset = self.tray[idx].asset.clone();
                self.switching_to = Some(id);
                self.open(vec![OpenRequest::Asset(asset)]);
            }
        }
    }

    pub fn remove_from_tray(&mut self, id: u64) {
        if self.current == Some(id) {
            let next = self.tray.iter().find(|t| t.id != id).map(|t| t.id);
            match next {
                Some(n) => self.switch_to(n),
                None => {
                    self.park_current();
                    self.current = None;
                }
            }
        }
        self.tray.retain(|t| t.id != id);
        self.job_photo.retain(|_, t| *t != id);
    }

    pub fn tray_step(&mut self, dir: i32) {
        let Some(cur) = self.switching_to.or(self.current) else { return };
        let Some(i) = self.tray.iter().position(|t| t.id == cur) else { return };
        let n = self.tray.len() as i32;
        if n > 1 {
            let next = self.tray[((i as i32 + dir).rem_euclid(n)) as usize].id;
            self.switch_to(next);
        }
    }

    // ------------------------------------------------------------ AI attribution

    pub(crate) fn track_job(&mut self, job: Job) {
        if let Some(cur) = self.current {
            self.job_photo.insert(job.id, cur);
        }
        self.jobs.push(job);
    }

    /// An AI result arrived for a photo that is parked as a stack.
    pub(crate) fn deliver_to_parked(&mut self, tray_id: u64, out: AiOutput) {
        let id = self.next_id;
        self.next_id += 1;
        let ctx = self.ctx.clone();
        let Some(item) = self.tray.iter_mut().find(|t| t.id == tray_id) else { return };
        let photo_name = item.asset.name.clone();
        let Some(session) = item.session.as_mut() else { return };
        let msg = match out {
            AiOutput::Director(report) => {
                let m = format!("Art direction for {photo_name} is ready: “{}”", report.title);
                session.director = Some(Director { report, thumbs: vec![], generation: 0 });
                m
            }
            AiOutput::Image { title, subtitle, engine, img, prompt, saved } => {
                let img = crate::app::cover_to(&Img::from_rgb8(&img), session.photo.work.w, session.photo.work.h);
                let img = Arc::new(img);
                let tex = ctx.load_texture(format!("art-{id}"), color_image(&img), TextureOptions::LINEAR);
                session.artworks.push(Artwork {
                    id,
                    title: title.clone(),
                    subtitle,
                    kind: Kind::Ai(engine),
                    base: img.clone(),
                    finish: Finish::default(),
                    display: img,
                    tex,
                    pinned: true,
                    placard: None,
                    recipe: None,
                    prompt: Some(prompt),
                    saved: Some(saved),
                    finish_gen: 0,
                });
                format!("“{title}” arrived for {photo_name} — it's waiting in its stack")
            }
            AiOutput::Placard { artwork_id, placard } => {
                if let Some(a) = session.artworks.iter_mut().find(|a| a.id == artwork_id) {
                    a.placard = Some(placard);
                }
                return;
            }
        };
        self.toast(msg, crate::theme::GOLD);
    }

    /// Texture for a recent's thumbnail, decoded on first use.
    pub fn recent_texture(&mut self, path: &PathBuf) -> Option<TextureHandle> {
        if let Some(t) = self.recent_tex.get(path) {
            return Some(t.clone());
        }
        let img = image::open(path).ok()?.to_rgb8();
        let tex = self.ctx.load_texture(
            format!("recent-{}", path.display()),
            ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw()),
            TextureOptions::LINEAR,
        );
        self.recent_tex.insert(path.clone(), tex.clone());
        Some(tex)
    }
}
