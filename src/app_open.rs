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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{Engine, Placard};
    use crate::app::test_support::*;

    fn img() -> Img {
        Img::new(8, 8, [0.5; 3])
    }

    #[test]
    fn opening_a_file_brings_it_on_stage_and_into_recents() {
        let mut s = studio();
        let asset = photo_asset("beach.png");
        s.app.open_sheet.visible = true;
        s.app.open(vec![OpenRequest::Input(Input::File(asset.local.clone()))]);
        assert!(!s.app.open_sheet.visible, "the sheet closes");
        assert_eq!(s.app.loading.as_ref().unwrap().label, "beach.png");
        s.until("the photo", |a| a.photo.is_some() && a.loading.is_none());
        let photo = s.app.photo.as_ref().unwrap();
        assert_eq!((photo.name.as_str(), photo.orig_size), ("beach.png", [96, 64]));
        assert_eq!(s.app.tray.len(), 1);
        assert_eq!(s.app.current, Some(s.app.tray[0].id));
        assert_eq!(s.app.recents.items[0].asset.name, "beach.png");
        assert!(s.app.recents.items[0].thumb.is_file(), "its thumbnail was saved");
        assert!(s.app.render_due.is_some() || s.app.render.is_some() || !s.app.artworks.is_empty(), "a first render follows");
    }

    #[test]
    fn opening_several_stages_the_first_and_stacks_the_rest() {
        let mut s = studio();
        let (a, b) = (photo_asset("a.png"), photo_asset("b.png"));
        s.app.open(vec![OpenRequest::Asset(a), OpenRequest::Asset(b)]);
        assert_eq!(s.app.loading.as_ref().unwrap().label, "a.png + 1 more");
        s.until("both photos", |a| a.loading.is_none() && a.tray.len() == 2);
        assert_eq!(s.app.photo.as_ref().unwrap().name, "a.png");
        assert!(s.app.tray.iter().all(|t| t.session.is_none()));
    }

    #[test]
    fn nothing_to_open_changes_nothing() {
        let mut s = studio();
        s.app.open(vec![]);
        assert!(s.app.loading.is_none());
    }

    #[test]
    fn a_newer_open_cancels_the_one_in_progress() {
        let mut s = studio();
        s.app.open(vec![OpenRequest::Asset(photo_asset("a.png"))]);
        let cancel = s.app.loading.as_ref().unwrap().cancel.clone();
        s.app.open(vec![OpenRequest::Asset(photo_asset("b.png"))]);
        assert!(cancel.load(Ordering::Relaxed));
        s.until("the second photo", |a| a.loading.is_none() && a.photo.is_some());
        assert_eq!(s.app.photo.as_ref().unwrap().name, "b.png");
    }

    #[test]
    fn photos_that_cannot_open_are_explained() {
        let mut s = studio();
        let junk = photo_asset("junk.png");
        std::fs::write(&junk.local, "not a picture").unwrap();
        let gone = Asset { local: "/no/such/gone.png".into(), ..photo_asset("gone.png") };
        s.app.open(vec![OpenRequest::Asset(junk), OpenRequest::Asset(gone), OpenRequest::Input(Input::File("/no/such/file.png".into()))]);
        s.until("the failures", |a| a.loading.is_none());
        assert!(s.app.photo.is_none());
        let toasts = s.toasts().join("\n");
        assert!(toasts.contains("Could not open junk.png"), "{toasts}");
        assert!(toasts.contains("gone.png is no longer available"), "{toasts}");
        assert!(toasts.contains("was not found"), "{toasts}");
    }

    #[test]
    fn pasted_text_opens_links_and_paths_or_explains_itself() {
        let mut s = studio();
        s.app.open_text("hello world");
        assert!(s.app.loading.is_none());
        assert_eq!(s.toasts().len(), 1);
        let asset = photo_asset("pasted.png");
        s.app.open_text(&format!("{}\n", asset.local.display()));
        s.until("the photo", |a| a.photo.is_some() && a.loading.is_none());
        assert_eq!(s.app.photo.as_ref().unwrap().name, "pasted.png");
    }

    #[test]
    fn recents_reopen_their_photo_and_forget_lost_ones() {
        let mut s = studio();
        s.app.open_recent(0); // Nothing there.
        assert!(s.app.loading.is_none());
        s.app.recents.push(photo_asset("kept.png"), "t.jpg".into());
        s.app.open_recent(0);
        s.until("the photo", |a| a.photo.is_some() && a.loading.is_none());
        assert_eq!(s.app.photo.as_ref().unwrap().name, "kept.png");

        let lost = Asset { local: "/no/such/lost.png".into(), name: "lost.png".into(), origin: crate::sources::Origin::Clipboard };
        s.app.recents.push(lost, "t.jpg".into());
        s.app.open_recent(0);
        assert_eq!(s.toasts().last().unwrap(), &"That pasted image is no longer cached");
        assert!(s.app.recents.items.iter().all(|r| r.asset.name != "lost.png"));
    }

    #[test]
    fn cancelling_stops_the_load_once() {
        let mut s = studio();
        s.app.cancel_loading();
        assert!(s.toasts().is_empty());
        s.app.open(vec![OpenRequest::Asset(photo_asset("a.png"))]);
        let cancel = s.app.loading.as_ref().unwrap().cancel.clone();
        s.app.cancel_loading();
        assert!(cancel.load(Ordering::Relaxed) && s.app.loading.is_none());
        assert_eq!(s.toasts(), ["Cancelled"]);
    }

    #[test]
    fn switching_photos_parks_and_restores_their_studio_state() {
        let mut s = studio();
        let first = s.with_photo("first.png");
        let art = s.app.add_artwork("Kept".into(), "".into(), Kind::Algorithm, img(), true);
        s.app.selected = Some(art);
        s.app.zoom = 3.0;
        let second = s.with_photo("second.png");
        assert_ne!(first, second);
        assert!(s.app.artworks.is_empty() && s.app.selected.is_none());
        assert_eq!(s.app.zoom, 1.0);

        s.app.switch_to(first);
        assert_eq!(s.app.current, Some(first));
        assert_eq!(s.app.photo.as_ref().unwrap().name, "first.png");
        assert_eq!((s.app.artworks[0].title.as_str(), s.app.selected), ("Kept", Some(art)));
        assert!(s.app.tray.iter().find(|t| t.id == second).unwrap().session.is_some(), "the other one is parked");
        assert!(s.app.tray.iter().find(|t| t.id == first).unwrap().session.is_none());
    }

    #[test]
    fn switching_cancels_the_render_in_progress() {
        let mut s = studio();
        let first = s.with_photo("first.png");
        s.with_photo("second.png");
        let cancel: Arc<AtomicBool> = Default::default();
        s.app.render = Some(crate::app::RenderJob { id: 900, style: "oil".into(), progress: Default::default(), cancel: cancel.clone() });
        s.app.switch_to(first);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(s.app.render.is_none());
    }

    #[test]
    fn a_photo_without_a_session_develops_again() {
        let mut s = studio();
        let asset = photo_asset("stack.png");
        s.app.on_acquired(asset, ColorImage::filled([2, 2], egui::Color32::RED), "t.jpg".into());
        let id = s.app.tray[0].id;
        s.app.switch_to(id);
        assert_eq!(s.app.switching_to, Some(id));
        s.until("the photo", |a| a.photo.is_some() && a.switching_to.is_none());
        assert_eq!(s.app.current, Some(id));
    }

    #[test]
    fn changing_your_mind_mid_switch_stays_on_the_current_photo() {
        let mut s = studio();
        let current = s.with_photo("current.png");
        s.app.on_acquired(photo_asset("other.png"), ColorImage::filled([2, 2], egui::Color32::RED), "t.jpg".into());
        let other = s.app.tray[1].id;
        s.app.switch_to(other);
        let cancel = s.app.loading.as_ref().unwrap().cancel.clone();
        s.app.switch_to(current);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(s.app.loading.is_none() && s.app.switching_to.is_none());
        s.app.switch_to(9999); // Unknown photos are ignored.
        assert_eq!(s.app.current, Some(current));
    }

    #[test]
    fn stepping_through_photos_wraps_around() {
        let mut s = studio();
        s.app.tray_step(1); // No photos.
        let a = s.with_photo("a.png");
        s.app.tray_step(1); // Only one: stays.
        assert_eq!(s.app.current, Some(a));
        let b = s.with_photo("b.png");
        let c = s.with_photo("c.png");
        s.app.tray_step(1);
        assert_eq!(s.app.current, Some(a), "past the end");
        s.app.tray_step(-1);
        assert_eq!(s.app.current, Some(c), "before the start");
        s.app.tray_step(-1);
        assert_eq!(s.app.current, Some(b));
    }

    #[test]
    fn removing_photos_moves_on_to_the_next_and_forgets_their_jobs() {
        let mut s = studio();
        let a = s.with_photo("a.png");
        let b = s.with_photo("b.png");
        s.app.job_photo.insert(5, b);
        s.app.remove_from_tray(b);
        assert_eq!(s.app.current, Some(a));
        assert!(s.app.job_photo.is_empty());
        assert_eq!(s.app.tray.len(), 1);
        s.app.remove_from_tray(a);
        assert_eq!(s.app.current, None);
        assert!(s.app.photo.is_none() && s.app.tray.is_empty());
    }

    #[test]
    fn opening_a_photo_again_reuses_its_tray_item() {
        let mut s = studio();
        let asset = photo_asset("again.png");
        let thumb = || ColorImage::filled([2, 2], egui::Color32::RED);
        s.app.on_acquired(asset.clone(), thumb(), "t1.jpg".into());
        s.app.on_acquired(asset.clone(), thumb(), "t2.jpg".into());
        assert_eq!(s.app.tray.len(), 1);
        assert_eq!(s.app.recents.items.len(), 1);
        assert_eq!(s.app.recents.items[0].thumb, PathBuf::from("t2.jpg"));
        let id = s.app.tray[0].id;
        let work = img();
        let color = color_image(&work);
        s.app.on_photo(asset, work.clone(), work, color, [8, 8]);
        assert_eq!(s.app.current, Some(id));
        assert_eq!(s.app.tray.len(), 1);
    }

    #[test]
    fn jobs_remember_the_photo_they_started_for() {
        let mut s = studio();
        let job = |id| crate::ai::Job::for_test(id, "J", Engine::Claude, crate::ai::JobStatus::Running);
        s.app.track_job(job(1));
        assert!(s.app.job_photo.is_empty(), "no photo on stage");
        let photo = s.with_photo("p.png");
        s.app.track_job(job(2));
        assert_eq!(s.app.job_photo[&2], photo);
        assert_eq!(s.app.jobs.len(), 2);
    }

    #[test]
    fn parked_photos_receive_every_kind_of_result() {
        let mut s = studio();
        let first = s.with_photo("first.png");
        let art = s.app.add_artwork("Old".into(), "".into(), Kind::Algorithm, img(), true);
        s.with_photo("second.png");

        s.app.deliver_to_parked(first, AiOutput::Director(report("Direction", &[])));
        s.app.deliver_to_parked(
            first,
            AiOutput::Image {
                title: "Dusk".into(),
                subtitle: "AI".into(),
                engine: Engine::Codex,
                img: image::RgbImage::new(20, 20),
                prompt: "p".into(),
                saved: "/tmp/s.png".into(),
            },
        );
        let label = Placard { title: "Label".into(), ..Default::default() };
        s.app.deliver_to_parked(first, AiOutput::Placard { artwork_id: art, placard: label });
        s.app.deliver_to_parked(9999, AiOutput::Director(report("Nobody", &[])));

        let session = s.app.tray.iter().find(|t| t.id == first).unwrap().session.as_ref().unwrap();
        assert_eq!(session.director.as_ref().unwrap().report.title, "Direction");
        assert_eq!(session.artworks[0].placard.as_ref().unwrap().title, "Label");
        let dusk = &session.artworks[1];
        assert_eq!((dusk.title.as_str(), dusk.display.w, dusk.display.h, dusk.pinned), ("Dusk", 96, 64, true));
        assert_eq!(
            s.toasts(),
            ["Art direction for first.png is ready: “Direction”", "“Dusk” arrived for first.png — it's waiting in its stack"]
        );

        // On return, its recipes get previews.
        s.app.tray.iter_mut().find(|t| t.id == first).unwrap().session.as_mut().unwrap().director =
            Some(Director { report: report("Direction", &[("A", "pixel")]), thumbs: vec![], generation: 0 });
        s.app.switch_to(first);
        s.until("the recipe preview", |a| a.director.as_ref().unwrap().thumbs.first().is_some_and(|t| t.is_some()));
    }

    #[test]
    fn recent_thumbnails_decode_once_and_tolerate_missing_files() {
        let mut s = studio();
        let path = photo_asset("thumb.png").local;
        assert!(s.app.recent_texture(&path).is_some());
        std::fs::remove_file(&path).unwrap();
        assert!(s.app.recent_texture(&path).is_some(), "served from memory");
        assert!(s.app.recent_texture(&PathBuf::from("/no/such/thumb.png")).is_none());
    }
}
