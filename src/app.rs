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
        crate::theme::install(&cc.egui_ctx);
        let (tx, rx) = channel();
        let (ai_tx, ai_rx) = channel();
        let ctx = cc.egui_ctx.clone();
        let mut app = Self {
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
            prefs: Prefs::load(),
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
            recents: Recents::load(),
            recent_tex: HashMap::new(),
            open_sheet: Default::default(),
            job_photo: HashMap::new(),
            film_focus: false,
            switching_to: None,
            plugin_scan: None,
            scanning_plugins: false,
            plugins_open: false,
            overview: Default::default(),
        };
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
