//! Application state, background workers and the message pump.
//! Drawing lives in `ui_canvas.rs` (the stage) and `ui_panels.rs` (chrome).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, atomic::AtomicBool, atomic::AtomicU32};

use egui::{ColorImage, TextureHandle, TextureOptions, Vec2};

use crate::ai::{self, AiEvent, AiHub, AiOutput, CliStatus, DirectorReport, Engine, Job, Placard, Prefs, Role, RoleCfg};
use crate::finish::Finish;
use crate::imaging::Img;
use crate::photo_io;
use crate::plugins;
use crate::plugins::{Ctx, Params};
use crate::sources::{self, Asset, Recents};

pub const THUMB_LONG: usize = 420;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Split,
    SideBySide,
    Single,
    Gallery,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LeftTab {
    Algorithms,
    AiStudio,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Algorithm,
    Ai(Engine),
}

pub struct Photo {
    pub path: PathBuf,
    pub name: String,
    /// Where it came from (file, link, clipboard) — shown in the UI and on wall labels.
    pub asset: Asset,
    pub work: Arc<Img>,
    pub thumb: Arc<Img>,
    pub tex: TextureHandle,
    pub orig_size: [u32; 2],
}

pub struct Artwork {
    pub id: u64,
    pub title: String,
    pub subtitle: String,
    pub kind: Kind,
    pub base: Arc<Img>,
    pub finish: Finish,
    pub display: Arc<Img>,
    pub tex: TextureHandle,
    pub pinned: bool,
    pub placard: Option<Placard>,
    /// Style id, settings and seed that produced an algorithmic artwork.
    pub recipe: Option<(String, Params, u64)>,
    pub prompt: Option<String>,
    pub saved: Option<PathBuf>,
    pub finish_gen: u64,
}

pub struct RenderJob {
    pub id: u64,
    pub style: String,
    pub progress: Arc<AtomicU32>,
    pub cancel: Arc<AtomicBool>,
}

pub struct Toast {
    pub text: String,
    pub color: egui::Color32,
    pub born: f64,
}

pub struct Reveal {
    pub artwork: u64,
    pub prev: Option<TextureHandle>,
    pub start: f64,
}

pub struct Director {
    pub report: DirectorReport,
    pub thumbs: Vec<Option<TextureHandle>>,
    pub generation: u64,
}

pub enum WorkerMsg {
    PhotoLoaded { batch: u64, result: Result<(Asset, Img, Img, ColorImage, [u32; 2]), String> },
    Acquiring { batch: u64, progress: sources::Progress },
    Acquired { asset: Asset, thumb: ColorImage, thumb_file: PathBuf },
    AcquireFailed { error: String },
    AcquireDone { batch: u64 },
    Clipboard(crate::app_open::ClipStatus),
    PluginsScanned(Box<plugins::external::Scan>),
    Thumb { generation: u64, style: String, img: ColorImage },
    RecipeThumb { generation: u64, idx: usize, img: ColorImage },
    Rendered { job: u64, style: String, params: Params, seed: u64, base: Arc<Img>, display: Arc<Img>, color: ColorImage },
    Finished { artwork: u64, generation: u64, display: Arc<Img>, color: ColorImage },
    RenderFailed { job: u64, style: String, error: String },
    Exported(Result<PathBuf, String>),
    FilesPicked(Vec<PathBuf>),
}

/// An open/download in progress, with live progress and cancellation.
pub struct LoadState {
    pub batch: u64,
    pub label: String,
    pub stage: String,
    pub done: u64,
    pub total: Option<u64>,
    pub cancel: Arc<AtomicBool>,
}

/// Everything that belongs to one photo, parked while another is on stage.
pub struct Session {
    pub photo: Photo,
    pub artworks: Vec<Artwork>,
    pub selected: Option<u64>,
    pub director: Option<Director>,
    pub thumbs: HashMap<String, TextureHandle>,
}

/// A photo opened in this session (shown as a stack when not on stage).
pub struct TrayItem {
    pub id: u64,
    pub asset: Asset,
    pub thumb: TextureHandle,
    pub session: Option<Session>,
}

pub struct App {
    pub ctx: egui::Context,
    pub photo: Option<Photo>,
    pub loading: Option<LoadState>,
    pub artworks: Vec<Artwork>,
    pub selected: Option<u64>,
    pub view: View,
    pub split: f32,
    pub zoom: f32,
    pub pan: Vec2,
    pub dragging_split: bool,
    pub left_tab: LeftTab,
    /// The style selected in the Atelier (a plug-in id).
    pub style_id: String,
    /// Current settings per style id, created from defaults on first use.
    pub params: HashMap<String, Params>,
    pub seed: u64,
    pub next_finish: Option<Finish>,
    pub render: Option<RenderJob>,
    pub render_due: Option<f64>,
    pub thumbs: HashMap<String, TextureHandle>,
    pub thumb_gen: u64,
    pub tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,
    pub hub: AiHub,
    ai_rx: Receiver<AiEvent>,
    cli_rx: Option<Receiver<(CliStatus, CliStatus)>>,
    pub claude: Option<CliStatus>,
    pub codex: Option<CliStatus>,
    pub jobs: Vec<Job>,
    pub director: Option<Director>,
    pub paint_preset: usize,
    pub paint_brief: String,
    pub duet: bool,
    /// Who plays each AI role right now (None = no capable CLI installed).
    pub director_role: Option<RoleCfg>,
    pub painter_role: Option<RoleCfg>,
    pub prefs: Prefs,
    pub vector_style: usize,
    pub toasts: Vec<Toast>,
    pub reveal: Option<Reveal>,
    pub next_id: u64,
    pub exporting: bool,
    pub picking: bool,
    pub work_res: usize,
    pub pending_finish_for_render: Option<(u64, Finish)>,
    pub recipe_title: Option<String>,
    /// Every photo opened this session, with its parked studio state.
    pub tray: Vec<TrayItem>,
    /// Tray id of the photo on stage.
    pub current: Option<u64>,
    pub recents: Recents,
    pub recent_tex: HashMap<PathBuf, TextureHandle>,
    pub open_sheet: crate::app_open::OpenSheet,
    /// AI job id → tray id of the photo it was started for.
    pub job_photo: HashMap<u64, u64>,
    /// Scroll the collection bar to the photo on stage on the next frame.
    pub film_focus: bool,
    /// Photo being brought on stage (still decoding), so rapid navigation
    /// keeps moving instead of restarting from the photo currently shown.
    pub switching_to: Option<u64>,
    /// The latest plug-in scan (locations, statuses), shown in the Plug-ins panel.
    pub plugin_scan: Option<plugins::external::Scan>,
    pub scanning_plugins: bool,
    pub plugins_open: bool,
    pub overview: crate::app_open::Overview,
}

/// Scale to cover `w`×`h` and centre-crop, so AI results never get stretched.
pub(crate) fn cover_to(src: &Img, w: usize, h: usize) -> Img {
    let s = (w as f32 / src.w as f32).max(h as f32 / src.h as f32);
    let (sw, sh) = (((src.w as f32 * s).ceil() as usize).max(w), ((src.h as f32 * s).ceil() as usize).max(h));
    let scaled = src.resize_exact(sw, sh, image::imageops::FilterType::Lanczos3);
    let (ox, oy) = ((sw - w) / 2, (sh - h) / 2);
    Img { w, h, px: (0..w * h).map(|i| scaled.at(ox + i % w, oy + i / w)).collect() }
}

pub fn color_image(img: &Img) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.to_rgba8_bytes())
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<String>) -> Self {
        // eframe restores egui's state from the last run even though this app
        // no longer saves it (see `persist_egui_memory`); drop any left over
        // by earlier versions, such as scroll positions, but keep the options.
        cc.egui_ctx.memory_mut(|m| {
            let options = m.options.clone();
            *m = egui::Memory::default();
            m.options = options;
        });
        crate::theme::install(&cc.egui_ctx);
        let mut app = Self::idle(cc.egui_ctx.clone(), Prefs::load(), Recents::load());
        app.rescan_clis();
        app.rescan_plugins();
        if let Some(raw) = initial {
            match sources::classify(&raw) {
                Ok(input) => app.open(vec![crate::app_open::OpenRequest::Input(input)]),
                Err(e) => app.toast(format!("Could not open “{raw}”: {e}"), crate::theme::DANGER),
            }
        }
        app
    }

    /// The app before it looks outside: no CLIs detected, no plug-ins scanned.
    fn idle(ctx: egui::Context, prefs: Prefs, recents: Recents) -> Self {
        let (tx, rx) = channel();
        let (ai_tx, ai_rx) = channel();
        Self {
            hub: AiHub::new(ai_tx, ctx.clone()),
            ctx,
            photo: None,
            loading: None,
            artworks: vec![],
            selected: None,
            view: View::Split,
            split: 0.5,
            zoom: 1.0,
            pan: Vec2::ZERO,
            dragging_split: false,
            left_tab: LeftTab::Algorithms,
            style_id: plugins::registry().first_id(),
            params: HashMap::new(),
            seed: 7,
            next_finish: None,
            render: None,
            render_due: None,
            thumbs: HashMap::new(),
            thumb_gen: 0,
            tx,
            rx,
            ai_rx,
            cli_rx: None,
            claude: None,
            codex: None,
            jobs: vec![],
            director: None,
            paint_preset: 0,
            paint_brief: ai::PAINT_PRESETS[0].prompt.to_string(),
            duet: false,
            director_role: None,
            painter_role: None,
            prefs,
            vector_style: 0,
            toasts: vec![],
            reveal: None,
            next_id: 1,
            exporting: false,
            picking: false,
            work_res: 2048,
            pending_finish_for_render: None,
            recipe_title: None,
            tray: vec![],
            current: None,
            recents,
            recent_tex: HashMap::new(),
            open_sheet: Default::default(),
            job_photo: HashMap::new(),
            film_focus: false,
            switching_to: None,
            plugin_scan: None,
            scanning_plugins: false,
            plugins_open: false,
            overview: Default::default(),
        }
    }

    pub fn now(&self) -> f64 {
        self.ctx.input(|i| i.time)
    }

    pub fn toast(&mut self, text: impl Into<String>, color: egui::Color32) {
        let born = self.now();
        self.toasts.push(Toast { text: text.into(), color, born });
    }

    pub fn selected_artwork(&self) -> Option<&Artwork> {
        self.selected.and_then(|id| self.artworks.iter().find(|a| a.id == id))
    }

    pub fn selected_artwork_mut(&mut self) -> Option<&mut Artwork> {
        let id = self.selected?;
        self.artworks.iter_mut().find(|a| a.id == id)
    }

    // ------------------------------------------------------------ photo

    pub(crate) fn render_thumbs(&mut self) {
        let Some(photo) = &self.photo else { return };
        self.thumb_gen += 1;
        self.thumbs.clear();
        let (generation, thumb, tx, ctx) = (self.thumb_gen, photo.thumb.clone(), self.tx.clone(), self.ctx.clone());
        let registry = plugins::registry();
        std::thread::spawn(move || {
            for plugin in registry.all() {
                let c = Ctx::for_image(&thumb, 7);
                if let Ok(img) = plugin.render(&thumb, &Params::defaults(plugin.description()), &c) {
                    let _ = tx.send(WorkerMsg::Thumb { generation, style: plugin.id().to_string(), img: color_image(&img) });
                    ctx.request_repaint();
                }
            }
        });
    }

    // ------------------------------------------------------------ rendering

    /// Settings for a style, initialised from its defaults on first use.
    pub fn params_for(&mut self, style: &str) -> &mut Params {
        self.params
            .entry(style.to_string())
            .or_insert_with(|| plugins::registry().get(style).map(|p| Params::defaults(p.description())).unwrap_or_default())
    }

    pub fn select_style(&mut self, id: &str) {
        self.style_id = id.to_string();
        self.render_due = Some(self.now());
        if self.selected_artwork().is_some_and(|a| a.kind != Kind::Algorithm) || self.view == View::Gallery {
            // Jump back to the algorithm draft so the change is visible.
            self.selected = self.draft_id();
        }
    }

    pub fn params_changed(&mut self) {
        self.render_due = Some(self.now() + 0.28);
    }

    pub fn draft_id(&self) -> Option<u64> {
        self.artworks.iter().find(|a| !a.pinned && a.kind == Kind::Algorithm).map(|a| a.id)
    }

    fn start_render(&mut self) {
        if self.photo.is_none() {
            return;
        }
        if let Some(r) = &self.render {
            r.cancel.store(true, Ordering::Relaxed);
        }
        let style = self.style_id.clone();
        let Some(plugin) = plugins::registry().get(&style).cloned() else { return };
        let params = self.params_for(&style).clone();
        let Some(photo) = &self.photo else { return };
        let seed = self.seed;
        let finish = self.next_finish.take().unwrap_or_else(|| {
            self.draft_id().and_then(|id| self.artworks.iter().find(|a| a.id == id)).map(|a| a.finish).unwrap_or_default()
        });
        let img = photo.work.clone();
        let ctx = Ctx::for_image(&img, seed);
        let id = self.next_id;
        self.next_id += 1;
        self.render = Some(RenderJob { id, style: style.clone(), progress: ctx.progress.clone(), cancel: ctx.cancel.clone() });
        let (tx, ectx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            match plugin.render(&img, &params, &ctx) {
                Ok(out) => {
                    let display = finish.apply(&out);
                    let color = color_image(&display);
                    let _ = tx.send(WorkerMsg::Rendered {
                        job: id,
                        style,
                        params,
                        seed,
                        base: Arc::new(out),
                        display: Arc::new(display),
                        color,
                    });
                }
                Err(plugins::RenderError::Cancelled) => {}
                Err(plugins::RenderError::Failed(error)) => {
                    let _ = tx.send(WorkerMsg::RenderFailed { job: id, style, error });
                }
            }
            ectx.request_repaint();
        });
        self.pending_finish_for_render = Some((id, finish));
    }

    pub fn refinish_selected(&mut self) {
        let Some(a) = self.selected_artwork_mut() else { return };
        a.finish_gen += 1;
        let (id, generation, base, finish) = (a.id, a.finish_gen, a.base.clone(), a.finish);
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let display = finish.apply(&base);
            let color = color_image(&display);
            let _ = tx.send(WorkerMsg::Finished { artwork: id, generation, display: Arc::new(display), color });
            ctx.request_repaint();
        });
    }

    pub fn add_artwork(&mut self, title: String, subtitle: String, kind: Kind, img: Img, pinned: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let img = Arc::new(img);
        let tex = self.ctx.load_texture(format!("art-{id}"), color_image(&img), TextureOptions::LINEAR);
        self.artworks.push(Artwork {
            id,
            title,
            subtitle,
            kind,
            base: img.clone(),
            finish: Finish::default(),
            display: img,
            tex,
            pinned,
            placard: None,
            recipe: None,
            prompt: None,
            saved: None,
            finish_gen: 0,
        });
        id
    }

    pub fn select(&mut self, id: Option<u64>) {
        self.selected = id;
        if let Some(a) = self.selected_artwork()
            && let Some((style, params, seed)) = a.recipe.clone()
        {
            self.params.insert(style.clone(), params);
            self.style_id = style;
            self.seed = seed;
        }
    }

    pub fn pin_draft(&mut self) {
        if let Some(id) = self.draft_id()
            && let Some(a) = self.artworks.iter_mut().find(|a| a.id == id)
        {
            a.pinned = true;
            let t = a.title.clone();
            self.toast(format!("Kept “{t}” in the collection"), crate::theme::GOLD);
        }
    }

    pub fn delete_artwork(&mut self, id: u64) {
        self.artworks.retain(|a| a.id != id);
        if self.selected == Some(id) {
            self.selected = self.artworks.last().map(|a| a.id);
        }
    }

    pub fn navigate(&mut self, dir: i32) {
        let mut ids: Vec<Option<u64>> = vec![None];
        ids.extend(self.artworks.iter().map(|a| Some(a.id)));
        let cur = ids.iter().position(|i| *i == self.selected).unwrap_or(0) as i32;
        let next = (cur + dir).clamp(0, ids.len() as i32 - 1) as usize;
        self.select(ids[next]);
    }

    // ------------------------------------------------------------ export

    pub fn export_selected(&mut self) {
        let (Some(photo), Some(a)) = (&self.photo, self.selected_artwork()) else {
            self.toast("Select an artwork to export", crate::theme::MUTED);
            return;
        };
        if self.exporting {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let base = std::path::Path::new(&photo.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let stem = format!("{base}-{}", a.title);
        let finish = a.finish;
        let job: Box<dyn FnOnce() -> anyhow::Result<PathBuf> + Send> = match (a.kind, a.recipe.clone()) {
            (Kind::Algorithm, Some((style, params, seed))) => {
                // Re-render at the photo's full resolution for print-quality output.
                let path = photo.path.clone();
                let plugin = plugins::registry().get(&style).cloned();
                Box::new(move || {
                    let plugin = plugin.ok_or_else(|| anyhow::anyhow!("style {style} is no longer available"))?;
                    let full = Img::from_rgb8(&photo_io::load_photo(&path)?).fit_long(6000);
                    let c = Ctx::for_image(&full, seed);
                    let out = plugin.render(&full, &params, &c)?;
                    photo_io::save_unique(&finish.apply(&out).to_rgb8(), &photo_io::output_dir(), &stem)
                })
            }
            _ => {
                let display = a.display.clone();
                Box::new(move || photo_io::save_unique(&display.to_rgb8(), &photo_io::output_dir(), &stem))
            }
        };
        self.exporting = true;
        self.toast("Exporting at full resolution…", crate::theme::GOLD);
        std::thread::spawn(move || {
            let r = job().map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerMsg::Exported(r));
            ctx.request_repaint();
        });
    }

    /// Save a time-lapse GIF of the selected algorithmic artwork being
    /// painted (BL-004), re-rendered with its style, settings and seed.
    pub fn export_timelapse(&mut self) {
        let (Some(photo), Some(a)) = (&self.photo, self.selected_artwork()) else {
            self.toast("Select an artwork to export", crate::theme::MUTED);
            return;
        };
        let Some((style, params, seed)) = a.recipe.clone().filter(|_| a.kind == Kind::Algorithm) else {
            self.toast("Time-lapses are made from algorithm styles", crate::theme::MUTED);
            return;
        };
        if self.exporting {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let base = std::path::Path::new(&photo.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let stem = format!("{base}-{}", a.title);
        let finish = a.finish;
        let path = photo.path.clone();
        let plugin = plugins::registry().get(&style).cloned();
        self.exporting = true;
        self.toast("Painting the time-lapse…", crate::theme::GOLD);
        std::thread::spawn(move || {
            let job = || -> anyhow::Result<PathBuf> {
                let plugin = plugin.ok_or_else(|| anyhow::anyhow!("style {style} is no longer available"))?;
                let img = Img::from_rgb8(&photo_io::load_photo(&path)?).fit_long(crate::timelapse::LONG_SIDE);
                crate::timelapse::export(plugin.as_ref(), &img, &params, seed, &finish, &photo_io::output_dir(), &stem)
            };
            let r = job().map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerMsg::Exported(r));
            ctx.request_repaint();
        });
    }

    pub fn open_output_folder(&self) {
        let dir = photo_io::output_dir();
        let _ = std::fs::create_dir_all(&dir);
        open_in_file_manager(&dir);
    }

    // ------------------------------------------------------------ plug-ins

    /// Look for external plug-ins in every location, off the UI thread.
    pub fn rescan_plugins(&mut self) {
        if self.scanning_plugins {
            return;
        }
        self.scanning_plugins = true;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let scan = plugins::external::scan(&plugins::Registry::builtin_ids(), &plugins::external::Trust::load());
            let _ = tx.send(WorkerMsg::PluginsScanned(Box::new(scan)));
            ctx.request_repaint();
        });
    }

    fn on_plugins_scanned(&mut self, scan: plugins::external::Scan) {
        self.scanning_plugins = false;
        let before: Vec<String> = plugins::registry().all().iter().map(|p| p.id().to_string()).collect();
        plugins::install(plugins::Registry::with_external(&scan));
        let registry = plugins::registry();
        let after: Vec<String> = registry.all().iter().map(|p| p.id().to_string()).collect();
        if registry.get(&self.style_id).is_none() {
            self.style_id = registry.first_id();
        }
        if before != after && self.photo.is_some() {
            self.render_thumbs();
        }
        let pending = scan.needing_approval();
        if pending > 0 && !self.plugins_open {
            let s = if pending == 1 { "plug-in needs" } else { "plug-ins need" };
            self.toast(format!("{pending} {s} your approval: open Plug-ins to review"), crate::theme::GOLD);
        }
        self.plugin_scan = Some(scan);
    }

    /// Approve a plug-in as it is now (pinned to its checksum) and rescan.
    pub fn approve_plugin(&mut self, dir: &std::path::Path, checksum: &str) {
        let mut trust = plugins::external::Trust::load();
        trust.approve(dir, checksum);
        match trust.save() {
            Ok(()) => self.rescan_plugins(),
            Err(e) => self.toast(format!("Could not save the approval: {e}"), crate::theme::DANGER),
        }
    }

    pub fn revoke_plugin(&mut self, dir: &std::path::Path) {
        let mut trust = plugins::external::Trust::load();
        trust.revoke(dir);
        match trust.save() {
            Ok(()) => self.rescan_plugins(),
            Err(e) => self.toast(format!("Could not save: {e}"), crate::theme::DANGER),
        }
    }

    /// Create (if needed) and open the user's plug-ins folder.
    pub fn open_user_plugins_folder(&mut self) {
        let Some(dir) = plugins::external::user_folder() else { return };
        match std::fs::create_dir_all(&dir) {
            Ok(()) => open_in_file_manager(&dir),
            Err(e) => self.toast(format!("Could not create {}: {e}", dir.display()), crate::theme::DANGER),
        }
    }

    // ------------------------------------------------------------ AI

    pub fn ai_busy(&self, engine_title: &str) -> bool {
        self.jobs.iter().any(|j| j.title == engine_title && j.shared.lock().unwrap().status == ai::JobStatus::Running)
    }

    /// (Re)discover which CLIs are installed, what they can do and which
    /// models they offer, then re-validate the role assignments.
    pub fn rescan_clis(&mut self) {
        if self.cli_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.cli_rx = Some(rx);
        self.claude = None;
        self.codex = None;
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let c = std::thread::spawn(ai::detect_claude);
            let x = ai::detect_codex();
            let _ = tx.send((c.join().unwrap_or_default(), x));
            ctx.request_repaint();
        });
    }

    pub fn cli_status(&self, cli: ai::Cli) -> Option<&CliStatus> {
        match cli {
            ai::Cli::Claude => self.claude.as_ref(),
            ai::Cli::Codex => self.codex.as_ref(),
        }
    }

    fn resolve_roles(&mut self) {
        self.director_role = self.prefs.resolve(Role::Director, self.claude.as_ref(), self.codex.as_ref());
        self.painter_role = self.prefs.resolve(Role::Painter, self.claude.as_ref(), self.codex.as_ref());
    }

    pub fn role(&self, role: Role) -> Option<&RoleCfg> {
        match role {
            Role::Director => self.director_role.as_ref(),
            Role::Painter => self.painter_role.as_ref(),
        }
    }

    /// The user picked a CLI and/or model for a role: apply and remember it.
    pub fn set_role(&mut self, role: Role, cfg: RoleCfg) {
        match role {
            Role::Director => {
                self.prefs.director = Some(cfg.clone());
                self.director_role = Some(cfg);
            }
            Role::Painter => {
                self.prefs.painter = Some(cfg.clone());
                self.painter_role = Some(cfg);
            }
        }
        self.prefs.save();
    }

    pub fn run_director(&mut self) {
        let (Some(photo), Some(d)) = (&self.photo, &self.director_role) else { return };
        let job = self.hub.art_director(d, photo.work.clone());
        self.track_job(job);
    }

    pub fn run_repaint(&mut self, medium: String, brief: String, with_brief: bool) {
        let (Some(photo), Some(p)) = (&self.photo, &self.painter_role) else { return };
        let director = if with_brief { self.director_role.as_ref() } else { None };
        let job = self.hub.repaint(p, director, photo.work.clone(), medium, brief);
        let msg = match p.cli {
            ai::Cli::Codex => "Codex is painting — usually 1–2 minutes",
            ai::Cli::Claude => "Claude is painting in SVG — usually 1–3 minutes",
        };
        let color = crate::theme::engine_color(job.engine);
        self.track_job(job);
        self.toast(msg, color);
    }

    pub fn run_vector(&mut self) {
        let (Some(photo), Some(d)) = (&self.photo, &self.director_role) else { return };
        let job = self.hub.vector(d, photo.work.clone(), &ai::VECTOR_STYLES[self.vector_style]);
        self.track_job(job);
    }

    pub fn run_placard(&mut self) {
        let Some(a) = self.selected_artwork() else { return };
        let how = match a.kind {
            Kind::Algorithm => format!("rendered with the '{}' algorithm", a.title),
            Kind::Ai(_) => a.subtitle.clone(),
        };
        let (id, img) = (a.id, a.display.clone());
        let Some(d) = &self.director_role else { return };
        let job = self.hub.placard(d, id, img, how);
        self.track_job(job);
    }

    pub fn apply_recipe(&mut self, idx: usize) {
        let Some(d) = &self.director else { return };
        let r = &d.report.recipes[idx];
        let registry = plugins::registry();
        let Some(plugin) = registry.get(&r.style) else {
            self.toast(format!("Unknown style “{}”", r.style), crate::theme::DANGER);
            return;
        };
        let params = Params::sanitized(plugin.description(), &r.params);
        let style = plugin.id().to_string();
        self.next_finish = Some(r.finish.sanitized());
        let name = r.name.clone();
        self.params.insert(style.clone(), params);
        self.style_id = style;
        self.recipe_title = Some(name);
        self.selected = self.draft_id();
        self.render_due = Some(self.now());
    }

    pub(crate) fn render_recipe_thumbs(&mut self) {
        let (Some(photo), Some(d)) = (&self.photo, &mut self.director) else { return };
        d.generation += 1;
        let generation = d.generation;
        let recipes: Vec<_> = d
            .report
            .recipes
            .iter()
            .map(|r| {
                plugins::registry().get(&r.style).map(|p| (p.clone(), Params::sanitized(p.description(), &r.params), r.finish.sanitized()))
            })
            .collect();
        d.thumbs = vec![None; recipes.len()];
        let (thumb, tx, ctx) = (photo.thumb.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            for (idx, r) in recipes.into_iter().enumerate() {
                let Some((plugin, params, finish)) = r else { continue };
                let c = Ctx::for_image(&thumb, 7);
                if let Ok(img) = plugin.render(&thumb, &params, &c) {
                    let _ = tx.send(WorkerMsg::RecipeThumb { generation, idx, img: color_image(&finish.apply(&img)) });
                    ctx.request_repaint();
                }
            }
        });
    }

    // ------------------------------------------------------------ pump

    pub fn pump(&mut self) {
        let now = self.now();
        if let Some(rx) = &self.cli_rx
            && let Ok((c, x)) = rx.try_recv()
        {
            self.claude = Some(c);
            self.codex = Some(x);
            self.cli_rx = None;
            self.resolve_roles();
        }
        if let (Some(due), true) = (self.render_due, self.photo.is_some())
            && now >= due
        {
            self.render_due = None;
            self.start_render();
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.on_worker(msg);
        }
        while let Ok(ev) = self.ai_rx.try_recv() {
            self.on_ai(ev);
        }
        let ttl = 5.0;
        self.toasts.retain(|t| now - t.born < ttl);
    }

    fn on_worker(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::FilesPicked(paths) => {
                self.picking = false;
                let reqs = paths.into_iter().map(|p| crate::app_open::OpenRequest::Input(sources::Input::File(p))).collect();
                self.open(reqs);
            }
            WorkerMsg::PhotoLoaded { batch, result } => match result {
                // A newer open superseded this one (e.g. rapid navigation): ignore it.
                Ok(_) if self.loading.as_ref().is_none_or(|l| l.batch != batch) => {}
                Ok((asset, work, thumb, color, orig)) => self.on_photo(asset, work, thumb, color, orig),
                Err(e) => {
                    if self.loading.as_ref().is_some_and(|l| l.batch == batch) {
                        self.loading = None;
                    }
                    self.toast(format!("Could not open photo: {e}"), crate::theme::DANGER)
                }
            },
            WorkerMsg::Acquiring { batch, progress } => {
                if let Some(l) = self.loading.as_mut().filter(|l| l.batch == batch) {
                    l.stage = progress.stage;
                    l.done = progress.done;
                    l.total = progress.total;
                }
            }
            WorkerMsg::Acquired { asset, thumb, thumb_file } => self.on_acquired(asset, thumb, thumb_file),
            WorkerMsg::AcquireFailed { error } => {
                if !error.contains("cancelled") {
                    self.toast(error, crate::theme::DANGER);
                }
            }
            WorkerMsg::AcquireDone { batch } => {
                if self.loading.as_ref().is_some_and(|l| l.batch == batch) {
                    self.loading = None;
                    self.switching_to = None;
                }
                self.recents.save();
            }
            WorkerMsg::Clipboard(status) => self.open_sheet.clip = status,
            WorkerMsg::PluginsScanned(scan) => self.on_plugins_scanned(*scan),
            WorkerMsg::Thumb { generation, style, img } => {
                if generation == self.thumb_gen {
                    let tex = self.ctx.load_texture(format!("thumb-{style}"), img, TextureOptions::LINEAR);
                    self.thumbs.insert(style, tex);
                }
            }
            WorkerMsg::RecipeThumb { generation, idx, img } => {
                let tex = self.ctx.load_texture(format!("recipe-{idx}"), img, TextureOptions::LINEAR);
                if let Some(d) = &mut self.director
                    && d.generation == generation
                    && idx < d.thumbs.len()
                {
                    d.thumbs[idx] = Some(tex);
                }
            }
            WorkerMsg::RenderFailed { job, style, error } => {
                if self.render.as_ref().map(|r| r.id) == Some(job) {
                    self.render = None;
                    let name = plugins::registry().get(&style).map(|p| p.description().name.clone()).unwrap_or(style);
                    self.toast(format!("{name} failed: {}", error.chars().take(220).collect::<String>()), crate::theme::DANGER);
                }
            }
            WorkerMsg::Rendered { job, style, params, seed, base, display, color } => {
                if self.render.as_ref().map(|r| r.id) != Some(job) {
                    return;
                }
                self.render = None;
                let finish = match self.pending_finish_for_render.take() {
                    Some((id, f)) if id == job => f,
                    _ => Finish::default(),
                };
                let style_name = plugins::registry().get(&style).map(|p| p.description().name.clone()).unwrap_or_else(|| style.clone());
                let title = self.recipe_title.take().unwrap_or_else(|| style_name.clone());
                let subtitle = format!("Algorithm · {style_name}");
                let now = self.now();
                let tex = self.ctx.load_texture(format!("draft-{job}"), color, TextureOptions::LINEAR);
                let id = match self.draft_id() {
                    Some(id) => {
                        let a = self.artworks.iter_mut().find(|a| a.id == id).unwrap();
                        let prev = std::mem::replace(&mut a.tex, tex);
                        a.title = title;
                        a.subtitle = subtitle;
                        a.base = base;
                        a.display = display;
                        a.finish = finish;
                        a.recipe = Some((style, params, seed));
                        a.placard = None;
                        a.finish_gen += 1;
                        self.reveal = Some(Reveal { artwork: id, prev: Some(prev), start: now });
                        id
                    }
                    None => {
                        let id = self.next_id;
                        self.next_id += 1;
                        self.artworks.push(Artwork {
                            id,
                            title,
                            subtitle,
                            kind: Kind::Algorithm,
                            base,
                            finish,
                            display,
                            tex,
                            pinned: false,
                            placard: None,
                            recipe: Some((style, params, seed)),
                            prompt: None,
                            saved: None,
                            finish_gen: 0,
                        });
                        self.reveal = Some(Reveal { artwork: id, prev: None, start: now });
                        id
                    }
                };
                // Whatever was on stage, a fresh render is what the user asked to see.
                self.selected = Some(id);
            }
            WorkerMsg::Finished { artwork, generation, display, color } => {
                if let Some(a) = self.artworks.iter_mut().find(|a| a.id == artwork)
                    && a.finish_gen == generation
                {
                    a.display = display;
                    a.tex.set(color, TextureOptions::LINEAR);
                }
            }
            WorkerMsg::Exported(r) => {
                self.exporting = false;
                match r {
                    Ok(p) => self.toast(format!("Saved {}", p.display()), crate::theme::GOLD),
                    Err(e) => self.toast(format!("Export failed: {e}"), crate::theme::DANGER),
                }
            }
        }
    }

    fn on_ai(&mut self, ev: AiEvent) {
        let title = self.jobs.iter().find(|j| j.id == ev.job_id).map(|j| j.title.clone()).unwrap_or_default();
        // Learn which models work on this account.
        match &ev.result {
            Ok(_) => {
                let before = self.prefs.unavailable.len();
                for k in &ev.models {
                    self.prefs.unavailable.remove(k);
                }
                if self.prefs.unavailable.len() != before {
                    self.prefs.save();
                }
            }
            Err(e) => {
                if let Some((key, reason)) = ai::parse_model_error(e) {
                    self.toast(format!("{key} is unavailable: {reason}. Pick another model under Roles."), crate::theme::DANGER);
                    self.prefs.unavailable.insert(key, reason);
                    self.prefs.save();
                    return;
                }
            }
        }
        let target = self.job_photo.get(&ev.job_id).copied();
        if let (Some(t), Ok(_)) = (target, &ev.result)
            && Some(t) != self.current
        {
            if let Ok(out) = ev.result {
                self.deliver_to_parked(t, out);
            }
            return;
        }
        match ev.result {
            Err(e) => {
                if !e.contains("cancelled") {
                    self.toast(format!("{title} failed: {}", e.chars().take(160).collect::<String>()), crate::theme::DANGER);
                }
            }
            Ok(AiOutput::Director(report)) => {
                let c = self.director_role.as_ref().map(|r| crate::theme::engine_color(r.engine())).unwrap_or(crate::theme::CLAUDE);
                self.toast(format!("Art direction ready: “{}”", report.title), c);
                if !report.paint_prompt.is_empty() {
                    self.paint_brief = report.paint_prompt.clone();
                }
                self.director = Some(Director { report, thumbs: vec![], generation: 0 });
                self.render_recipe_thumbs();
            }
            Ok(AiOutput::Image { title, subtitle, engine, img, prompt, saved }) => {
                let Some(photo) = &self.photo else { return };
                // Match the photo's working size so split comparisons line up.
                let img = cover_to(&Img::from_rgb8(&img), photo.work.w, photo.work.h);
                let id = self.add_artwork(title.clone(), subtitle, Kind::Ai(engine), img, true);
                if let Some(a) = self.artworks.iter_mut().find(|a| a.id == id) {
                    a.prompt = Some(prompt);
                    a.saved = Some(saved);
                }
                self.selected = Some(id);
                self.reveal = Some(Reveal { artwork: id, prev: None, start: self.now() });
                self.toast(format!("“{title}” has arrived"), crate::theme::engine_color(engine));
            }
            Ok(AiOutput::Placard { artwork_id, placard }) => {
                if let Some(a) = self.artworks.iter_mut().find(|a| a.id == artwork_id) {
                    a.placard = Some(placard);
                }
            }
        }
    }
}

impl eframe::App for App {
    /// Start every session with fresh panels (scroll positions, open
    /// sections) rather than egui's state from the last run; `App::new`
    /// clears state saved by earlier versions. The window's size and
    /// position are persisted separately and still restored.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();
        self.shortcuts(ui.ctx());
        self.draw(ui);
        let busy = self.render.is_some()
            || self.loading.is_some()
            || self.render_due.is_some()
            || self.reveal.is_some()
            || !self.toasts.is_empty()
            || self.photo.is_none()
            || self.jobs.iter().any(|j| j.shared.lock().unwrap().status == ai::JobStatus::Running);
        if busy {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        }
    }
}

/// Reveal a folder in the platform's file manager.
pub fn open_in_file_manager(dir: &std::path::Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A headless studio for tests: the real app and UI on an `egui::Context`
    //! with no window. It never runs the AI CLIs, reads the clipboard or opens
    //! dialogs, and `photo_io::user_dir` keeps its files out of the user's.

    use std::sync::atomic::AtomicU64;

    use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Shape};

    use super::*;

    #[derive(Debug)]
    pub struct DroppedPath(pub std::path::PathBuf);

    impl egui::DroppedFile for DroppedPath {
        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    pub struct Studio {
        pub app: App,
        pub size: [f32; 2],
        pub texts: Vec<(String, Rect)>,
        time: f64,
    }

    fn collect(shape: &Shape, clip: Rect, out: &mut Vec<(String, Rect)>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, clip, out)),
            Shape::Text(t) => {
                let rect = t.galley.rect.translate(t.pos.to_vec2());
                if clip.intersects(rect) && !t.galley.text().trim().is_empty() {
                    out.push((t.galley.text().to_string(), rect.intersect(clip)));
                }
            }
            _ => {}
        }
    }

    /// An empty studio that has drawn its first frame.
    pub fn studio() -> Studio {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let app = App::idle(ctx, Prefs::default(), Recents::default());
        let mut s = Studio { app, size: [1600.0, 980.0], texts: vec![], time: 1.0 };
        s.frame();
        s
    }

    /// A photo file of its own, as a file-origin asset.
    pub fn photo_asset(name: &str) -> Asset {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = photo_io::user_dir(None).join("photos").join(NEXT.fetch_add(1, Ordering::Relaxed).to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        image::RgbImage::from_fn(96, 64, |x, y| image::Rgb([(x * 2) as u8, (y * 3) as u8, 90])).save(&path).unwrap();
        let path = photo_io::canonicalize(&path).unwrap();
        Asset { local: path.clone(), name: name.into(), origin: sources::Origin::File { path } }
    }

    pub fn test_img() -> Img {
        Img::from_rgb8(&image::RgbImage::from_fn(96, 64, |x, y| image::Rgb([(x * 2) as u8, (y * 3) as u8, 90])))
    }

    pub fn report(title: &str, recipes: &[(&str, &str)]) -> DirectorReport {
        let recipes: Vec<_> = recipes.iter().map(|(n, s)| serde_json::json!({"name": n, "style": s, "why": "because"})).collect();
        serde_json::from_value(serde_json::json!({
            "title": title, "reading": "A reading", "palette": ["#112233"], "recipes": recipes, "paint_prompt": "a brief",
        }))
        .unwrap()
    }

    impl Studio {
        /// Draw one frame with `events`, as `eframe::App::ui` would.
        pub fn frame_with(&mut self, events: Vec<Event>, modifiers: Modifiers, dropped: Vec<egui::DroppedFileHandle>) {
            self.time += 1.0 / 60.0;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size.into())),
                time: Some(self.time),
                predicted_dt: 1.0 / 60.0,
                events: std::iter::once(Event::ModifiersChanged(modifiers)).chain(events).collect(),
                dropped_files: dropped,
                ..Default::default()
            };
            let ctx = self.app.ctx.clone();
            let app = &mut self.app;
            let output = ctx.run_ui(raw, |ui| {
                app.pump();
                app.shortcuts(ui.ctx());
                app.draw(ui);
            });
            self.texts.clear();
            for clipped in &output.shapes {
                collect(&clipped.shape, clipped.clip_rect, &mut self.texts);
            }
        }

        pub fn frame(&mut self) {
            self.frame_with(vec![], Modifiers::NONE, vec![]);
        }

        /// Whether some text on screen contains `text`.
        pub fn shows(&self, text: &str) -> bool {
            self.texts.iter().any(|(t, _)| t.contains(text))
        }

        /// A key press and release with `modifiers`.
        pub fn press(&mut self, key: Key, modifiers: Modifiers) {
            let event = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
            self.frame_with(vec![event(true)], modifiers, vec![]);
            self.frame_with(vec![event(false)], modifiers, vec![]);
        }

        /// Click the topmost text on screen that equals `text`.
        pub fn click(&mut self, text: &str) {
            let rect = self.texts.iter().rev().find(|(t, _)| t == text).unwrap_or_else(|| panic!("no “{text}” in {:?}", self.labels())).1;
            let pos = rect.center();
            let button = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            self.frame_with(vec![Event::PointerMoved(pos)], Modifiers::NONE, vec![]);
            self.frame_with(vec![button(true)], Modifiers::NONE, vec![]);
            self.frame_with(vec![button(false)], Modifiers::NONE, vec![]);
            // Away again, so nothing stays hovered.
            self.frame_with(vec![Event::PointerGone], Modifiers::NONE, vec![]);
        }

        pub fn labels(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        /// Draw frames until `done`, waiting for background work.
        pub fn until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
            let started = std::time::Instant::now();
            while !done(&self.app) {
                assert!(started.elapsed().as_secs() < 120, "timed out waiting for {what}");
                std::thread::sleep(std::time::Duration::from_millis(5));
                self.frame();
            }
            self.frame();
        }

        /// Put a photo on stage, as a finished open does.
        pub fn with_photo(&mut self, name: &str) -> u64 {
            let asset = photo_asset(name);
            let work = test_img();
            let color = color_image(&work);
            self.app.on_photo(asset, work.clone(), work, color, [96, 64]);
            self.app.render_due = None;
            self.frame();
            self.app.current.unwrap()
        }

        /// A photo with its algorithm draft rendered and selected.
        pub fn with_draft(&mut self, name: &str) -> u64 {
            self.with_photo(name);
            self.app.render_due = Some(0.0);
            self.until("the draft", |a| a.draft_id().is_some() && a.render.is_none());
            self.app.draft_id().unwrap()
        }

        pub fn toasts(&self) -> Vec<&str> {
            self.app.toasts.iter().map(|t| t.text.as_str()).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::ai::{Cli, JobStatus, ModelInfo};
    use crate::sources::Origin;

    fn solid(w: usize, h: usize) -> Img {
        Img::new(w, h, [0.5; 3])
    }

    fn rendered(job: u64, style: &str) -> WorkerMsg {
        let img = Arc::new(solid(8, 8));
        WorkerMsg::Rendered {
            job,
            style: style.into(),
            params: Params::default(),
            seed: 3,
            base: img.clone(),
            display: img.clone(),
            color: color_image(&img),
        }
    }

    fn render_job(app: &mut App, id: u64) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        app.render = Some(RenderJob { id, style: "oil".into(), progress: Default::default(), cancel: cancel.clone() });
        cancel
    }

    fn event(result: Result<AiOutput, String>) -> AiEvent {
        AiEvent { job_id: 77, result, models: vec![] }
    }

    fn painted(title: &str) -> AiOutput {
        AiOutput::Image {
            title: title.into(),
            subtitle: "AI".into(),
            engine: Engine::Codex,
            img: image::RgbImage::from_pixel(40, 40, image::Rgb([9, 9, 9])),
            prompt: "a prompt".into(),
            saved: "/tmp/saved.png".into(),
        }
    }

    #[test]
    fn covering_scales_and_crops_without_stretching() {
        let mut src = solid(100, 50);
        // A red band across the middle of the width, to see the crop.
        src = src.map_xy(|x, _, _| if (40..60).contains(&x) { [1.0, 0.0, 0.0] } else { [0.0; 3] });
        let out = cover_to(&src, 50, 50);
        assert_eq!((out.w, out.h), (50, 50));
        assert!(out.at(25, 25)[0] > 0.9, "the middle survives");
        assert!(out.at(1, 25)[0] < 0.2, "the sides are cropped, not squeezed in");
        let same = cover_to(&solid(30, 20), 30, 20);
        assert_eq!((same.w, same.h), (30, 20));
    }

    #[test]
    fn a_new_app_is_ready_with_the_first_style() {
        let s = studio();
        assert!(s.app.photo.is_none() && s.app.artworks.is_empty());
        assert_eq!(s.app.style_id, "oil");
        assert_eq!(s.app.paint_brief, ai::PAINT_PRESETS[0].prompt);
    }

    #[test]
    fn photos_from_an_older_open_are_ignored() {
        let mut s = studio();
        s.app.loading =
            Some(LoadState { batch: 2, label: "x".into(), stage: String::new(), done: 0, total: None, cancel: Default::default() });
        let work = test_img();
        let color = color_image(&work);
        let msg =
            WorkerMsg::PhotoLoaded { batch: 1, result: Ok((photo_asset("old.png"), work.clone(), work.clone(), color.clone(), [96, 64])) };
        s.app.on_worker(msg);
        assert!(s.app.photo.is_none());
        assert!(s.app.loading.is_some());
        let msg = WorkerMsg::PhotoLoaded { batch: 2, result: Ok((photo_asset("new.png"), work.clone(), work, color, [96, 64])) };
        s.app.on_worker(msg);
        assert_eq!(s.app.photo.as_ref().unwrap().name, "new.png");
    }

    #[test]
    fn a_failed_open_clears_only_its_own_progress_and_says_why() {
        let mut s = studio();
        s.app.loading =
            Some(LoadState { batch: 2, label: "x".into(), stage: String::new(), done: 0, total: None, cancel: Default::default() });
        s.app.on_worker(WorkerMsg::PhotoLoaded { batch: 1, result: Err("old".into()) });
        assert!(s.app.loading.is_some());
        s.app.on_worker(WorkerMsg::PhotoLoaded { batch: 2, result: Err("corrupt".into()) });
        assert!(s.app.loading.is_none());
        assert_eq!(s.toasts(), ["Could not open photo: old", "Could not open photo: corrupt"]);
    }

    #[test]
    fn download_progress_only_updates_its_own_batch() {
        let mut s = studio();
        s.app.loading =
            Some(LoadState { batch: 2, label: "x".into(), stage: "start".into(), done: 0, total: None, cancel: Default::default() });
        let progress = |stage: &str| sources::Progress { stage: stage.into(), done: 5, total: Some(10) };
        s.app.on_worker(WorkerMsg::Acquiring { batch: 1, progress: progress("other") });
        assert_eq!(s.app.loading.as_ref().unwrap().stage, "start");
        s.app.on_worker(WorkerMsg::Acquiring { batch: 2, progress: progress("mine") });
        let l = s.app.loading.as_ref().unwrap();
        assert_eq!((l.stage.as_str(), l.done, l.total), ("mine", 5, Some(10)));
    }

    #[test]
    fn a_cancelled_download_fails_quietly() {
        let mut s = studio();
        s.app.on_worker(WorkerMsg::AcquireFailed { error: "download cancelled".into() });
        assert!(s.toasts().is_empty());
        s.app.on_worker(WorkerMsg::AcquireFailed { error: "That link is broken".into() });
        assert_eq!(s.toasts(), ["That link is broken"]);
    }

    #[test]
    fn style_previews_from_an_older_photo_are_ignored() {
        let mut s = studio();
        s.app.thumb_gen = 5;
        let img = color_image(&solid(4, 4));
        s.app.on_worker(WorkerMsg::Thumb { generation: 4, style: "oil".into(), img: img.clone() });
        assert!(s.app.thumbs.is_empty());
        s.app.on_worker(WorkerMsg::Thumb { generation: 5, style: "oil".into(), img });
        assert!(s.app.thumbs.contains_key("oil"));
    }

    #[test]
    fn recipe_previews_land_on_the_current_report_only() {
        let mut s = studio();
        s.app.director = Some(Director { report: report("T", &[("A", "oil")]), thumbs: vec![None], generation: 3 });
        let img = color_image(&solid(4, 4));
        s.app.on_worker(WorkerMsg::RecipeThumb { generation: 2, idx: 0, img: img.clone() });
        assert!(s.app.director.as_ref().unwrap().thumbs[0].is_none());
        s.app.on_worker(WorkerMsg::RecipeThumb { generation: 3, idx: 9, img: img.clone() });
        s.app.on_worker(WorkerMsg::RecipeThumb { generation: 3, idx: 0, img });
        assert!(s.app.director.as_ref().unwrap().thumbs[0].is_some());
    }

    #[test]
    fn a_render_replaces_the_draft_and_kept_artworks_stay() {
        let mut s = studio();
        render_job(&mut s.app, 10);
        s.app.recipe_title = Some("Moonlit".into());
        s.app.on_worker(rendered(10, "oil"));
        let first = s.app.draft_id().unwrap();
        assert!(s.app.render.is_none());
        assert_eq!(s.app.selected, Some(first));
        let a = s.app.selected_artwork().unwrap();
        assert_eq!(a.title, "Moonlit", "the recipe names the artwork");
        assert!(a.subtitle.starts_with("Algorithm · "));
        assert!(s.app.reveal.as_ref().unwrap().prev.is_none());

        render_job(&mut s.app, 11);
        s.app.on_worker(rendered(11, "halftone"));
        assert_eq!(s.app.artworks.len(), 1, "the draft is reused");
        let a = s.app.selected_artwork().unwrap();
        assert_ne!(a.title, "Moonlit", "the recipe's name was for one render");
        assert_eq!(a.finish_gen, 1);
        assert_eq!(a.recipe.as_ref().unwrap().0, "halftone");
        assert!(s.app.reveal.as_ref().unwrap().prev.is_some(), "the old picture cross-fades");

        s.app.pin_draft();
        assert_eq!(s.toasts().last().unwrap(), &format!("Kept “{}” in the collection", s.app.artworks[0].title));
        render_job(&mut s.app, 12);
        s.app.on_worker(rendered(12, "pixel"));
        assert_eq!(s.app.artworks.len(), 2, "a kept artwork is never replaced");
        assert_ne!(s.app.draft_id(), Some(first));
    }

    #[test]
    fn results_of_superseded_renders_are_dropped() {
        let mut s = studio();
        render_job(&mut s.app, 10);
        s.app.on_worker(rendered(9, "oil"));
        assert!(s.app.artworks.is_empty());
        assert!(s.app.render.is_some());
        s.app.on_worker(WorkerMsg::RenderFailed { job: 9, style: "oil".into(), error: "old".into() });
        assert!(s.app.render.is_some() && s.toasts().is_empty());
    }

    #[test]
    fn a_failed_render_names_the_style_and_bounds_the_message() {
        let mut s = studio();
        render_job(&mut s.app, 10);
        s.app.on_worker(WorkerMsg::RenderFailed { job: 10, style: "oil".into(), error: "x".repeat(500) });
        assert!(s.app.render.is_none());
        let name = plugins::registry().get("oil").unwrap().description().name.clone();
        let toast = s.toasts()[0].to_string();
        assert!(toast.starts_with(&format!("{name} failed: ")));
        assert_eq!(toast.matches('x').count(), 220);
    }

    #[test]
    fn a_finish_applied_by_a_recipe_reaches_the_render_it_was_for() {
        let mut s = studio();
        let glow = Finish { glow: 0.5, ..Default::default() };
        render_job(&mut s.app, 10);
        s.app.pending_finish_for_render = Some((10, glow));
        s.app.on_worker(rendered(10, "oil"));
        assert_eq!(s.app.selected_artwork().unwrap().finish, glow);
        render_job(&mut s.app, 11);
        s.app.pending_finish_for_render = Some((99, glow));
        s.app.on_worker(rendered(11, "oil"));
        assert!(s.app.selected_artwork().unwrap().finish.is_identity());
    }

    #[test]
    fn only_the_latest_refinish_of_an_artwork_is_shown() {
        let mut s = studio();
        let id = s.app.add_artwork("T".into(), "S".into(), Kind::Algorithm, solid(4, 4), false);
        s.app.artworks[0].finish_gen = 2;
        let new = Arc::new(Img::new(4, 4, [1.0; 3]));
        let msg = |generation| WorkerMsg::Finished { artwork: id, generation, display: new.clone(), color: color_image(&new) };
        s.app.on_worker(msg(1));
        assert_eq!(s.app.artworks[0].display.at(0, 0), [0.5; 3]);
        s.app.on_worker(msg(2));
        assert_eq!(s.app.artworks[0].display.at(0, 0), [1.0; 3]);
    }

    #[test]
    fn saving_reports_where_the_file_went_or_why_it_failed() {
        let mut s = studio();
        s.app.exporting = true;
        s.app.on_worker(WorkerMsg::Exported(Ok("/pics/a.png".into())));
        assert!(!s.app.exporting);
        s.app.exporting = true;
        s.app.on_worker(WorkerMsg::Exported(Err("disk full".into())));
        assert!(!s.app.exporting);
        assert_eq!(s.toasts()[1], "Export failed: disk full");
        assert!(s.toasts()[0].starts_with("Saved "));
    }

    #[test]
    fn selecting_an_artwork_restores_its_style_settings_and_seed() {
        let mut s = studio();
        let id = s.app.add_artwork("T".into(), "S".into(), Kind::Algorithm, solid(4, 4), true);
        let mut params = Params::default();
        params.0.insert("k".into(), plugins::Value::Number(2.0));
        s.app.artworks[0].recipe = Some(("pixel".into(), params.clone(), 99));
        s.app.select(Some(id));
        assert_eq!((s.app.style_id.as_str(), s.app.seed), ("pixel", 99));
        assert_eq!(s.app.params["pixel"], params);
        s.app.select(None);
        assert!(s.app.selected_artwork().is_none());
        assert_eq!(s.app.style_id, "pixel", "the original photo leaves the style alone");
    }

    #[test]
    fn navigating_walks_from_the_photo_through_the_artworks_and_stops_at_the_ends() {
        let mut s = studio();
        let a = s.app.add_artwork("A".into(), "".into(), Kind::Algorithm, solid(4, 4), true);
        let b = s.app.add_artwork("B".into(), "".into(), Kind::Algorithm, solid(4, 4), true);
        s.app.navigate(-1);
        assert_eq!(s.app.selected, None);
        s.app.navigate(1);
        assert_eq!(s.app.selected, Some(a));
        s.app.navigate(1);
        s.app.navigate(1);
        assert_eq!(s.app.selected, Some(b));
    }

    #[test]
    fn deleting_the_selected_artwork_selects_the_last_one() {
        let mut s = studio();
        let a = s.app.add_artwork("A".into(), "".into(), Kind::Algorithm, solid(4, 4), true);
        let b = s.app.add_artwork("B".into(), "".into(), Kind::Algorithm, solid(4, 4), true);
        let c = s.app.add_artwork("C".into(), "".into(), Kind::Algorithm, solid(4, 4), true);
        s.app.selected = Some(a);
        s.app.delete_artwork(b);
        assert_eq!(s.app.selected, Some(a), "another selection is left alone");
        s.app.delete_artwork(a);
        assert_eq!(s.app.selected, Some(c));
        s.app.delete_artwork(c);
        assert_eq!(s.app.selected, None);
    }

    #[test]
    fn settings_start_from_each_styles_defaults() {
        let mut s = studio();
        let oil = plugins::registry().get("oil").unwrap().description().clone();
        assert_eq!(*s.app.params_for("oil"), Params::defaults(&oil));
        assert_eq!(*s.app.params_for("no-such-style"), Params::default());
    }

    #[test]
    fn choosing_a_style_jumps_back_to_the_draft_so_the_change_is_visible() {
        let mut s = studio();
        let draft = s.app.add_artwork("D".into(), "".into(), Kind::Algorithm, solid(4, 4), false);
        let ai = s.app.add_artwork("AI".into(), "".into(), Kind::Ai(Engine::Codex), solid(4, 4), true);
        s.app.selected = Some(ai);
        s.app.select_style("pixel");
        assert_eq!((s.app.style_id.as_str(), s.app.selected), ("pixel", Some(draft)));
        assert!(s.app.render_due.is_some());
        s.app.selected = None;
        s.app.view = View::Gallery;
        s.app.select_style("oil");
        assert_eq!(s.app.selected, Some(draft));
    }

    #[test]
    fn changing_settings_waits_a_moment_before_rendering() {
        let mut s = studio();
        s.app.params_changed();
        assert!(s.app.render_due.unwrap() > s.app.now());
    }

    #[test]
    fn a_new_render_stops_the_one_in_progress() {
        let mut s = studio();
        s.with_photo("p.png");
        let cancel = render_job(&mut s.app, 500);
        s.app.start_render();
        assert!(cancel.load(Ordering::Relaxed));
        assert_ne!(s.app.render.as_ref().unwrap().id, 500);
    }

    #[test]
    fn rendering_runs_from_the_chosen_style_to_a_selected_artwork() {
        let mut s = studio();
        s.with_photo("p.png");
        s.app.select_style("pixel");
        s.until("the render", |a| a.selected_artwork().is_some_and(|x| x.recipe.as_ref().is_some_and(|r| r.0 == "pixel")));
        let a = s.app.selected_artwork().unwrap();
        assert_eq!((a.display.w, a.display.h), (96, 64));
        assert!(a.subtitle.ends_with(&plugins::registry().get("pixel").unwrap().description().name));
    }

    #[test]
    fn refinishing_applies_the_finish_in_the_background() {
        let mut s = studio();
        let id = s.app.add_artwork("T".into(), "".into(), Kind::Algorithm, solid(16, 16), true);
        s.app.selected = Some(id);
        s.app.selected_artwork_mut().unwrap().finish = Finish { exposure: 1.0, ..Default::default() };
        s.app.refinish_selected();
        s.until("the finish", |a| a.artworks[0].display.at(0, 0)[0] > 0.6);
        assert_eq!(s.app.artworks[0].base.at(0, 0), [0.5; 3], "the base stays untouched");
    }

    #[test]
    fn saving_needs_a_selected_artwork() {
        let mut s = studio();
        s.app.export_selected();
        s.app.export_timelapse();
        assert_eq!(s.toasts(), ["Select an artwork to export", "Select an artwork to export"]);
        assert!(!s.app.exporting);
    }

    #[test]
    fn saving_writes_the_artwork_in_the_output_folder() {
        let mut s = studio();
        let id = s.with_draft("Saved Photo.png");
        s.app.artworks.iter_mut().find(|a| a.id == id).unwrap().title = "first".into();
        s.app.export_selected();
        assert!(s.app.exporting);
        assert_eq!(s.toasts().last().unwrap(), &"Exporting at full resolution…");
        s.app.export_selected(); // Ignored while one is under way.
        assert_eq!(s.toasts().iter().filter(|t| t.starts_with("Exporting")).count(), 1);
        s.until("the save", |a| !a.exporting);
        let toast = s.toasts().last().unwrap().to_string();
        let path = toast.strip_prefix("Saved ").unwrap_or_else(|| panic!("{toast}"));
        assert!(std::path::Path::new(path).starts_with(photo_io::output_dir()));
        assert_eq!(image::open(path).unwrap().width(), 96, "re-rendered from the photo's full size");
        assert!(path.contains("saved-photo-first"), "{path}");
    }

    #[test]
    fn an_ai_artwork_saves_as_it_is() {
        let mut s = studio();
        s.with_photo("p.png");
        let id = s.app.add_artwork("Painted".into(), "".into(), Kind::Ai(Engine::Codex), solid(20, 10), true);
        s.app.selected = Some(id);
        s.app.export_selected();
        s.until("the save", |a| !a.exporting);
        let toast = s.toasts().last().unwrap().to_string();
        assert_eq!(image::open(toast.strip_prefix("Saved ").unwrap()).unwrap().width(), 20);
    }

    #[test]
    fn a_save_that_fails_says_so() {
        let mut s = studio();
        let id = s.with_draft("p.png");
        // The style vanished since the artwork was made.
        s.app.artworks.iter_mut().find(|a| a.id == id).unwrap().recipe.as_mut().unwrap().0 = "gone".into();
        s.app.export_selected();
        s.until("the failure", |a| !a.exporting);
        assert!(s.toasts().last().unwrap().contains("style gone is no longer available"));
    }

    #[test]
    fn time_lapses_come_from_algorithm_artworks_only() {
        let mut s = studio();
        s.with_photo("p.png");
        let id = s.app.add_artwork("AI".into(), "".into(), Kind::Ai(Engine::Claude), solid(4, 4), true);
        s.app.selected = Some(id);
        s.app.export_timelapse();
        assert_eq!(s.toasts().last().unwrap(), &"Time-lapses are made from algorithm styles");
        assert!(!s.app.exporting);
    }

    #[test]
    fn a_time_lapse_is_saved_as_a_gif() {
        let mut s = studio();
        s.app.style_id = "pixel".into();
        s.with_draft("p.png");
        s.app.export_timelapse();
        assert!(s.app.exporting);
        s.app.export_timelapse(); // Ignored while one is under way.
        s.until("the time-lapse", |a| !a.exporting);
        let toast = s.toasts().last().unwrap().to_string();
        let path = toast.strip_prefix("Saved ").unwrap_or_else(|| panic!("{toast}"));
        assert!(path.ends_with(".gif") && std::fs::read(path).unwrap().starts_with(b"GIF"), "{path}");
    }

    #[test]
    fn recipes_set_up_the_next_render() {
        let mut s = studio();
        s.app.apply_recipe(0); // No report yet: nothing happens.
        assert!(s.app.render_due.is_none());
        let draft = s.app.add_artwork("D".into(), "".into(), Kind::Algorithm, solid(4, 4), false);
        let mut report = report("T", &[("Moody", "pixel"), ("Lost", "no-such-style")]);
        report.recipes[0].params.insert("size".into(), plugins::Value::Number(10_000.0));
        report.recipes[0].finish = Finish { grain: 9.0, ..Default::default() };
        s.app.director = Some(Director { report, thumbs: vec![], generation: 0 });
        s.app.apply_recipe(0);
        assert_eq!((s.app.style_id.as_str(), s.app.recipe_title.as_deref()), ("pixel", Some("Moody")));
        assert_eq!(s.app.selected, Some(draft));
        assert!(s.app.render_due.is_some());
        assert_eq!(s.app.next_finish.unwrap().grain, 1.0, "AI values are clamped");
        let size = s.app.params["pixel"].get("size");
        assert!(size < 10_000.0, "AI values are clamped: {size}");

        s.app.apply_recipe(1);
        assert_eq!(s.toasts().last().unwrap(), &"Unknown style “no-such-style”");
        assert_eq!(s.app.style_id, "pixel");
    }

    #[test]
    fn jobs_need_a_photo_and_a_role_before_they_start() {
        let mut s = studio();
        s.app.run_director();
        s.app.run_vector();
        s.app.run_repaint("Oil".into(), "brief".into(), false);
        s.app.run_placard();
        assert!(s.app.jobs.is_empty());
        s.with_photo("p.png");
        s.app.run_director();
        s.app.run_vector();
        s.app.run_repaint("Oil".into(), "brief".into(), true);
        s.app.run_placard();
        assert!(s.app.jobs.is_empty(), "no CLI holds a role");
    }

    #[test]
    fn busy_means_a_running_job_of_that_title() {
        let mut s = studio();
        s.app.jobs.push(ai::Job::for_test(1, "Art direction", Engine::Claude, JobStatus::Running));
        s.app.jobs.push(ai::Job::for_test(2, "Repaint", Engine::Codex, JobStatus::Done));
        assert!(s.app.ai_busy("Art direction"));
        assert!(!s.app.ai_busy("Repaint"));
        assert!(!s.app.ai_busy("Other"));
    }

    fn cli(models: &[&str], image_gen: bool) -> CliStatus {
        let models = models.iter().map(|m| ModelInfo { id: m.to_string(), ..Default::default() }).collect();
        CliStatus { found: true, version: "1.0".into(), image_gen, models, default_model: None }
    }

    #[test]
    fn discovered_clis_fill_the_roles() {
        let mut s = studio();
        let (tx, rx) = channel();
        s.app.cli_rx = Some(rx);
        s.app.rescan_clis(); // One already running: no second detection.
        assert!(s.app.claude.is_none());
        tx.send((cli(&["opus"], false), cli(&[], true))).unwrap();
        s.frame();
        assert!(s.app.cli_rx.is_none());
        assert_eq!(s.app.role(Role::Director).unwrap().cli, Cli::Claude);
        assert_eq!(s.app.role(Role::Painter).unwrap().cli, Cli::Codex);
        assert!(s.app.cli_status(Cli::Claude).unwrap().found && s.app.cli_status(Cli::Codex).unwrap().image_gen);
    }

    /// The one test that writes `prefs.json`, so it can read it back.
    #[test]
    fn model_health_and_role_choices_are_remembered() {
        let mut s = studio();
        let failed = "job: MODEL_UNAVAILABLE[claude:fable]: requires usage credits".to_string();
        s.app.on_ai(event(Err(failed)));
        assert_eq!(s.toasts()[0], "claude:fable is unavailable: requires usage credits. Pick another model under Roles.");
        assert_eq!(Prefs::load().unavailable["claude:fable"], "requires usage credits");

        // A success with that model clears the mark.
        s.app.on_ai(AiEvent { job_id: 77, result: Ok(painted("T")), models: vec!["claude:fable".into()] });
        assert!(Prefs::load().unavailable.is_empty());

        let choice = RoleCfg { cli: Cli::Codex, model: "gpt-x".into() };
        s.app.set_role(Role::Director, choice.clone());
        s.app.set_role(Role::Painter, choice.clone());
        assert_eq!(s.app.role(Role::Director), Some(&choice));
        assert_eq!(s.app.role(Role::Painter), Some(&choice));
        let saved = Prefs::load();
        assert_eq!((saved.director, saved.painter), (Some(choice.clone()), Some(choice)));
    }

    #[test]
    fn failed_jobs_say_so_unless_they_were_cancelled() {
        let mut s = studio();
        s.app.jobs.push(ai::Job::for_test(77, "Art direction", Engine::Claude, JobStatus::Running));
        s.app.on_ai(event(Err("cancelled by the user".into())));
        assert!(s.toasts().is_empty());
        s.app.on_ai(event(Err("y".repeat(400))));
        let toast = s.toasts()[0];
        assert!(toast.starts_with("Art direction failed: "));
        assert_eq!(toast.matches('y').count(), 160);
    }

    #[test]
    fn art_direction_fills_the_brief_and_renders_its_recipes() {
        let mut s = studio();
        s.with_photo("p.png");
        s.app.on_ai(event(Ok(AiOutput::Director(report("Golden hour", &[("A", "pixel"), ("B", "no-such-style")])))));
        assert_eq!(s.toasts().last().unwrap(), &"Art direction ready: “Golden hour”");
        assert_eq!(s.app.paint_brief, "a brief");
        s.until("the recipe preview", |a| a.director.as_ref().unwrap().thumbs[0].is_some());
        assert!(s.app.director.as_ref().unwrap().thumbs[1].is_none(), "an unknown style has no preview");
    }

    #[test]
    fn a_painting_arrives_at_the_photos_size_and_on_stage() {
        let mut s = studio();
        s.with_photo("p.png");
        s.app.on_ai(event(Ok(painted("Dusk"))));
        let a = s.app.selected_artwork().unwrap();
        assert_eq!((a.title.as_str(), a.kind, a.pinned), ("Dusk", Kind::Ai(Engine::Codex), true));
        assert_eq!((a.display.w, a.display.h), (96, 64));
        assert_eq!(a.prompt.as_deref(), Some("a prompt"));
        assert_eq!(a.saved.as_deref(), Some(std::path::Path::new("/tmp/saved.png")));
        assert!(s.app.reveal.is_some());
        assert_eq!(s.toasts().last().unwrap(), &"“Dusk” has arrived");
    }

    #[test]
    fn a_painting_with_no_photo_on_stage_is_dropped() {
        let mut s = studio();
        s.app.on_ai(event(Ok(painted("Dusk"))));
        assert!(s.app.artworks.is_empty());
    }

    #[test]
    fn wall_labels_reach_their_artwork() {
        let mut s = studio();
        let id = s.app.add_artwork("T".into(), "".into(), Kind::Algorithm, solid(4, 4), true);
        let placard =
            |artwork_id| Ok(AiOutput::Placard { artwork_id, placard: ai::Placard { title: "Label".into(), ..Default::default() } });
        s.app.on_ai(event(placard(id + 100)));
        assert!(s.app.artworks[0].placard.is_none());
        s.app.on_ai(event(placard(id)));
        assert_eq!(s.app.artworks[0].placard.as_ref().unwrap().title, "Label");
    }

    #[test]
    fn results_for_a_parked_photo_wait_in_its_stack() {
        let mut s = studio();
        let first = s.with_photo("first.png");
        s.app.job_photo.insert(77, first);
        s.with_photo("second.png");
        s.app.on_ai(event(Ok(painted("Dusk"))));
        assert!(s.app.artworks.is_empty(), "not on the photo now on stage");
        let session = s.app.tray.iter().find(|t| t.id == first).unwrap().session.as_ref().unwrap();
        assert_eq!(session.artworks[0].title, "Dusk");
    }

    #[test]
    fn plugin_scans_update_the_status_and_point_out_pending_approvals() {
        let mut s = studio();
        let root = photo_io::user_dir(None).join(format!("scan-{}", std::process::id()));
        for name in ["a", "b"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            let toml = format!("protocol = 1\nid = \"pending-{name}\"\nname = \"{name}\"\nrun = [\"never\"]\n");
            std::fs::write(root.join(name).join("plugin.toml"), toml).unwrap();
        }
        let location = plugins::external::Location { kind: plugins::external::LocationKind::Env, path: root.clone(), exists: true };
        let builtin = plugins::Registry::builtin_ids();
        let scan = plugins::external::scan_locations(vec![location], &builtin, &Default::default());
        s.app.scanning_plugins = true;
        s.app.style_id = "gone-style".into();
        s.app.on_worker(WorkerMsg::PluginsScanned(Box::new(scan.clone())));
        assert!(!s.app.scanning_plugins);
        assert_eq!(s.app.style_id, "oil", "a style that left falls back to the first");
        assert_eq!(s.toasts(), ["2 plug-ins need your approval: open Plug-ins to review"]);
        assert_eq!(s.app.plugin_scan.as_ref().unwrap().found.len(), 2);

        s.app.plugins_open = true;
        s.app.on_plugins_scanned(scan);
        assert_eq!(s.toasts().len(), 1, "no need to point it out with the panel open");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn toasts_fade_after_a_few_seconds() {
        let mut s = studio();
        s.app.toast("hello", crate::theme::GOLD);
        s.frame();
        s.frame();
        assert!(s.shows("hello"));
        s.app.toasts[0].born -= 10.0;
        s.frame();
        assert!(s.app.toasts.is_empty());
    }

    #[test]
    fn file_origins_show_on_stage() {
        let mut s = studio();
        s.with_photo("p.png");
        assert!(matches!(s.app.photo.as_ref().unwrap().asset.origin, Origin::File { .. }));
        assert_eq!(s.app.photo.as_ref().unwrap().orig_size, [96, 64]);
    }
}
