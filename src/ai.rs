//! AI back-ends. Everything goes through the locally installed, already
//! authenticated `claude` and `codex` CLIs (your subscriptions) — no API keys.
//!
//! Work is split into two *roles*, each assignable to whichever CLI is
//! installed, with its own model:
//!
//! * **Art Director** — vision → words: art direction (recipes for the
//!   algorithms), briefs for the Master Painter, vector reinterpretations (SVG)
//!   and gallery wall labels.
//! * **Master Painter** — repaints the whole photo. Codex paints raster images
//!   with its image-generation tool; Claude Code has no raster image model,
//!   so as Master Painter Claude paints in SVG.
//!
//! Which CLIs exist, whether Codex can generate images, and which models each
//! offers are discovered when the program starts (and on "Rescan").

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::finish::Finish;
use crate::imaging::Img;
use crate::photo_io;

// ------------------------------------------------------------------ CLIs & models

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Cli {
    Claude,
    Codex,
}

impl Cli {
    pub const ALL: [Cli; 2] = [Cli::Claude, Cli::Codex];
    pub fn name(self) -> &'static str {
        match self {
            Cli::Claude => "Claude",
            Cli::Codex => "Codex",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Cli::Claude => "claude",
            Cli::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, Default)]
pub struct CliStatus {
    pub found: bool,
    pub version: String,
    /// Codex only: the `image_generation` feature exists in this build.
    pub image_gen: bool,
    pub models: Vec<ModelInfo>,
    /// The model the CLI uses when none is passed (from its own config).
    pub default_model: Option<String>,
}

fn run_quiet(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).stdin(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn detect_claude() -> CliStatus {
    let Some(v) = run_quiet("claude", &["--version"]) else { return CliStatus::default() };
    // Claude Code has no "list models" command; it accepts these aliases for
    // the latest model of each family. Availability on the user's plan is
    // learned when a run fails (see `ModelHealth`).
    let models = [
        ("opus", "Opus", "Most capable — richest art direction"),
        ("sonnet", "Sonnet", "Balanced — fast and sharp"),
        ("haiku", "Haiku", "Fastest and lightest"),
        ("fable", "Fable", "Newest frontier model"),
    ]
    .iter()
    .map(|(id, label, d)| ModelInfo { id: id.to_string(), label: label.to_string(), description: d.to_string() })
    .collect();
    let default_model = dirs::home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".claude/settings.json")).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["model"].as_str().map(String::from));
    CliStatus { found: true, version: v.split_whitespace().next().unwrap_or("").to_string(), image_gen: false, models, default_model }
}

pub fn detect_codex() -> CliStatus {
    let Some(v) = run_quiet("codex", &["--version"]) else { return CliStatus::default() };
    let version = v.split_whitespace().last().unwrap_or("").to_string();
    let image_gen = run_quiet("codex", &["features", "list"])
        .map(|f| f.lines().any(|l| l.split_whitespace().next() == Some("image_generation")))
        .unwrap_or(false);
    let home = dirs::home_dir().unwrap_or_default().join(".codex");
    // Codex caches the model catalogue for the logged-in account.
    let models = std::fs::read_to_string(home.join("models_cache.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| {
            v["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["visibility"].as_str().is_none_or(|vis| vis == "list"))
                .filter_map(|m| {
                    let id = m["slug"].as_str()?.to_string();
                    Some(ModelInfo {
                        label: m["display_name"].as_str().unwrap_or(&id).to_string(),
                        description: m["description"].as_str().unwrap_or("").to_string(),
                        id,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let default_model = std::fs::read_to_string(home.join("config.toml")).ok().and_then(|cfg| {
        cfg.lines()
            .take_while(|l| !l.trim_start().starts_with('['))
            .find_map(|l| l.trim().strip_prefix("model").map(str::trim).and_then(|r| r.strip_prefix('=')))
            .map(|r| r.trim().trim_matches('"').to_string())
    });
    CliStatus { found: true, version, image_gen, models, default_model }
}

// ------------------------------------------------------------------ roles

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Director,
    Painter,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Director => "Art Director",
            Role::Painter => "Master Painter",
        }
    }
    /// Preferred CLI order when nothing (valid) was chosen before.
    fn preference(self) -> [Cli; 2] {
        match self {
            Role::Director => [Cli::Claude, Cli::Codex],
            Role::Painter => [Cli::Codex, Cli::Claude],
        }
    }
    /// Can `cli` (in its detected state) fill this role? `Err` says why not.
    pub fn eligible(self, cli: Cli, status: Option<&CliStatus>) -> Result<(), String> {
        match status {
            None => Err("still detecting…".into()),
            Some(s) if !s.found => Err(format!("`{}` CLI not found on PATH", cli.key())),
            Some(s) if self == Role::Painter && cli == Cli::Codex && !s.image_gen => {
                Err("this Codex build has no image_generation feature".into())
            }
            Some(_) => Ok(()),
        }
    }
}

/// Which CLI plays a role, and with which model ("" = the CLI's default).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoleCfg {
    pub cli: Cli,
    #[serde(default)]
    pub model: String,
}

impl RoleCfg {
    pub fn engine(&self) -> Engine {
        match self.cli {
            Cli::Claude => Engine::Claude,
            Cli::Codex => Engine::Codex,
        }
    }
    /// e.g. "Claude · opus" or "Codex · default".
    pub fn describe(&self) -> String {
        format!("{} · {}", self.cli.name(), if self.model.is_empty() { "default" } else { &self.model })
    }
    fn health_key(&self) -> String {
        format!("{}:{}", self.cli.key(), if self.model.is_empty() { "default" } else { &self.model })
    }
    pub fn health_key_for(cli: Cli, model: &str) -> String {
        RoleCfg { cli, model: model.to_string() }.health_key()
    }
}

/// Persisted choices plus what we've learned about model availability.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Prefs {
    pub director: Option<RoleCfg>,
    pub painter: Option<RoleCfg>,
    /// "cli:model" → reason it failed (e.g. "requires usage credits").
    #[serde(default)]
    pub unavailable: BTreeMap<String, String>,
}

impl Prefs {
    fn path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("photo-airt").join("prefs.json")
    }
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }

    /// Pick a valid assignment for `role` given what is installed right now:
    /// keep the saved choice when still possible, otherwise fall back to the
    /// role's preferred CLI and that CLI's default model.
    pub fn resolve(&self, role: Role, claude: Option<&CliStatus>, codex: Option<&CliStatus>) -> Option<RoleCfg> {
        let status = |cli| match cli {
            Cli::Claude => claude,
            Cli::Codex => codex,
        };
        let saved = match role {
            Role::Director => self.director.as_ref(),
            Role::Painter => self.painter.as_ref(),
        };
        if let Some(s) = saved
            && role.eligible(s.cli, status(s.cli)).is_ok()
        {
            let st = status(s.cli).unwrap();
            let model_ok = s.model.is_empty() || st.models.iter().any(|m| m.id == s.model);
            return Some(RoleCfg { cli: s.cli, model: if model_ok { s.model.clone() } else { String::new() } });
        }
        role.preference().into_iter().find(|&c| role.eligible(c, status(c)).is_ok()).map(|cli| RoleCfg { cli, model: String::new() })
    }
}

/// Recognise "this model can't be used" failures (as opposed to transient
/// errors) so the model can be flagged in the picker.
fn model_problem(text: &str) -> Option<String> {
    let t = text.to_lowercase();
    const MARKERS: [&str; 10] = [
        "requires usage credits",
        "model not found",
        "not_found_error",
        "invalid model",
        "unknown model",
        "model does not exist",
        "is not supported",
        "not available on your plan",
        "do not have access to",
        "model is not available",
    ];
    MARKERS.iter().any(|m| t.contains(m)).then(|| {
        let first = text.split(". ").next().unwrap_or(text).lines().next().unwrap_or(text).trim().trim_end_matches('.');
        first.chars().take(90).collect()
    })
}

const MODEL_ERR: &str = "MODEL_UNAVAILABLE";

/// Parse an error produced by a runner for an unusable model.
pub fn parse_model_error(err: &str) -> Option<(String, String)> {
    let rest = &err[err.find(MODEL_ERR)? + MODEL_ERR.len()..];
    let rest = rest.strip_prefix('[')?;
    let end = rest.find(']')?;
    let reason = rest[end + 1..].trim_start_matches(':').trim().to_string();
    Some((rest[..end].to_string(), reason))
}

// ------------------------------------------------------------------ jobs

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    Claude,
    Codex,
    Duet,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobStatus {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

pub struct JobShared {
    pub log: Vec<String>,
    pub stage: String,
    pub status: JobStatus,
    pub ended: Option<Instant>,
    child: Option<Child>,
    cancelled: bool,
}

pub struct Job {
    pub id: u64,
    pub title: String,
    pub engine: Engine,
    pub started: Instant,
    pub shared: Arc<Mutex<JobShared>>,
}

impl Job {
    pub fn cancel(&self) {
        let mut s = self.shared.lock().unwrap();
        s.cancelled = true;
        if let Some(c) = s.child.as_mut() {
            let _ = c.kill();
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Recipe {
    pub name: String,
    pub style: String,
    #[serde(default)]
    pub why: String,
    /// Setting values by key: numbers, booleans (toggles) or strings (choices).
    #[serde(default)]
    pub params: BTreeMap<String, crate::plugins::Value>,
    #[serde(default)]
    pub finish: Finish,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DirectorReport {
    pub title: String,
    pub reading: String,
    #[serde(default)]
    pub palette: Vec<String>,
    #[serde(default)]
    pub recipes: Vec<Recipe>,
    #[serde(default, alias = "codex_prompt")]
    pub paint_prompt: String,
    #[serde(default, alias = "codex_medium")]
    pub paint_medium: String,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct Placard {
    pub title: String,
    #[serde(default)]
    pub medium: String,
    #[serde(default)]
    pub year: String,
    #[serde(default)]
    pub note: String,
}

pub enum AiOutput {
    Director(DirectorReport),
    Image { title: String, subtitle: String, engine: Engine, img: image::RgbImage, prompt: String, saved: PathBuf },
    Placard { artwork_id: u64, placard: Placard },
}

pub struct AiEvent {
    pub job_id: u64,
    pub result: Result<AiOutput, String>,
    /// Health keys ("cli:model") of the models this job used.
    pub models: Vec<String>,
}

/// Spawns jobs on background threads and reports back over a channel.
pub struct AiHub {
    tx: Sender<AiEvent>,
    ctx: egui::Context,
    next_id: u64,
}

pub struct PaintPreset {
    pub name: &'static str,
    pub prompt: &'static str,
}

pub const PAINT_PRESETS: &[PaintPreset] = &[
    PaintPreset {
        name: "Impasto Oil",
        prompt: "a loose impressionist oil painting with thick impasto brush strokes, palette-knife texture and luminous light",
    },
    PaintPreset {
        name: "Watercolour",
        prompt: "a delicate watercolor on cold-press paper with soft transparent washes, pigment blooms, pooled edges, a fine pencil underdrawing and an irregular white deckled border with a few paint splatters",
    },
    PaintPreset {
        name: "Ukiyo-e",
        prompt: "a Japanese ukiyo-e woodblock print with flat colour areas, carved outlines, bokashi gradients and washi paper texture",
    },
    PaintPreset {
        name: "Starry Night",
        prompt: "a Post-Impressionist painting with swirling, rhythmic, directional brush strokes and vivid complementary colour, in the manner of Van Gogh",
    },
    PaintPreset {
        name: "Charcoal",
        prompt: "an expressive charcoal and white chalk drawing on toned paper with smudged shadows and confident gestural lines",
    },
    PaintPreset {
        name: "Gouache Poster",
        prompt: "a mid-century gouache travel poster with flat matte shapes, a limited palette and crisp graphic design, no text",
    },
    PaintPreset {
        name: "Soft Pastel",
        prompt: "a soft pastel drawing with powdery layered strokes on textured pastel paper, in the manner of Degas",
    },
    PaintPreset {
        name: "Art Nouveau",
        prompt: "an Art Nouveau illustration with flowing ornamental linework, decorative borders and muted jewel tones, in the manner of Alphonse Mucha",
    },
    PaintPreset {
        name: "Claymation",
        prompt: "a hand-made claymation diorama with sculpted plasticine textures, visible fingerprints and soft studio lighting",
    },
    PaintPreset { name: "Pixel Epic", prompt: "a richly detailed 16-bit pixel-art scene with a limited palette and careful dithering" },
];

pub struct VectorStyle {
    pub name: &'static str,
    pub brief: &'static str,
}

pub const VECTOR_STYLES: &[VectorStyle] = &[
    VectorStyle {
        name: "Matisse Cut-outs",
        brief: "Henri Matisse's late paper cut-outs: bold organic shapes of flat, saturated gouache-painted paper with slightly irregular scissor edges, no outlines",
    },
    VectorStyle {
        name: "Cubist",
        brief: "synthetic Cubism: the scene fractured into overlapping faceted planes seen from several viewpoints, earthy ochres and greys with a few bright accents",
    },
    VectorStyle {
        name: "Bauhaus",
        brief: "a Bauhaus geometric poster: the scene built only from circles, rectangles, arcs and bold lines in primary colours plus black on off-white paper",
    },
    VectorStyle {
        name: "Woodblock",
        brief: "a flat-colour Japanese woodblock print: layered silhouettes, linearGradient skies, thin dark outlines",
    },
    VectorStyle {
        name: "Line Art",
        brief: "an elegant minimalist continuous-line drawing in black ink on cream paper with two or three flat colour accents",
    },
    VectorStyle { name: "Mosaic", brief: "a Byzantine mosaic made of hundreds of small rectangular tesserae with dark grout gaps" },
];

/// How to point each CLI at the input image inside a prompt.
fn look_at(cfg: &RoleCfg, path: &Path) -> String {
    match cfg.cli {
        Cli::Claude => format!("Read the image file {}", path.display()),
        Cli::Codex => format!("Study the attached image ({})", path.display()),
    }
}

/// Run a vision → text task on whichever CLI holds the role.
fn run_text(cfg: &RoleCfg, prompt: &str, dir: &Path, image: &Path, sh: &Arc<Mutex<JobShared>>, ctx: &egui::Context) -> Result<String> {
    match cfg.cli {
        Cli::Claude => run_claude(prompt, dir, &cfg.model, sh, ctx),
        Cli::Codex => run_codex(prompt, dir, &[image.to_path_buf()], &cfg.model, false, sh, ctx).map(|(_, msg)| msg),
    }
}

impl AiHub {
    pub fn new(tx: Sender<AiEvent>, ctx: egui::Context) -> Self {
        Self { tx, ctx, next_id: 1 }
    }

    fn spawn(
        &mut self,
        title: &str,
        engine: Engine,
        roles: &[&RoleCfg],
        work: impl FnOnce(&Arc<Mutex<JobShared>>, PathBuf) -> Result<AiOutput> + Send + 'static,
    ) -> Job {
        let id = self.next_id;
        self.next_id += 1;
        let shared = Arc::new(Mutex::new(JobShared {
            log: vec![],
            stage: "Starting…".into(),
            status: JobStatus::Running,
            ended: None,
            child: None,
            cancelled: false,
        }));
        let models: Vec<String> = roles.iter().map(|r| r.health_key()).collect();
        let (tx, ctx, sh) = (self.tx.clone(), self.ctx.clone(), shared.clone());
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let dir = photo_io::cache_dir().join("jobs").join(format!("{stamp}-{id}-{}", photo_io::slug(title)));
        std::thread::spawn(move || {
            let res = std::fs::create_dir_all(&dir).map_err(anyhow::Error::from).and_then(|_| work(&sh, dir));
            let mut s = sh.lock().unwrap();
            s.ended = Some(Instant::now());
            s.status = match &res {
                Ok(_) => JobStatus::Done,
                Err(_) if s.cancelled => JobStatus::Cancelled,
                Err(e) => JobStatus::Failed(format!("{e:#}").replace(MODEL_ERR, "Model unavailable")),
            };
            let result = res.map_err(|e| format!("{e:#}"));
            drop(s);
            let _ = tx.send(AiEvent { job_id: id, result, models });
            ctx.request_repaint();
        });
        Job { id, title: title.to_string(), engine, started: Instant::now(), shared }
    }

    /// The director studies the photo and writes recipes for our algorithms.
    pub fn art_director(&mut self, director: &RoleCfg, photo: Arc<Img>) -> Job {
        let ctx = self.ctx.clone();
        let cfg = director.clone();
        self.spawn("Art direction", cfg.engine(), &[director], move |sh, dir| {
            let input = write_input(&photo, &dir, "photo.jpg")?;
            let prompt = format!(
                "You are the art director of Photo·AIrt, an app that turns photographs into artworks.\n\
                 {look} and study it closely: subject, light, palette, mood, composition.\n\n\
                 The app renders these algorithmic styles (id, description, params with ranges):\n{cat}\n\
                 Optional global finish adjustments: exposure -1..1, contrast -1..1, saturation -1..1, warmth -1..1, \
                 vignette 0..1, grain 0..1, canvas 0..1, glow 0..1.\n\n\
                 Respond with ONLY one JSON object (no prose, no markdown fences, do not create files) of this shape:\n\
                 {{\"title\": \"an evocative title for the artwork-to-be\",\n \
                 \"reading\": \"2-3 sentences on what makes this photo special and how to treat it\",\n \
                 \"palette\": [\"#rrggbb\", 5 dominant colours],\n \
                 \"recipes\": [3 objects {{\"name\": \"short poetic name\", \"style\": \"<style id>\", \"why\": \"one sentence\", \
                 \"params\": {{...tuned values...}}, \"finish\": {{...}}}}, each using a DIFFERENT style id, tuned for this photo],\n \
                 \"paint_medium\": \"short name of the traditional medium you would choose\",\n \
                 \"paint_prompt\": \"a vivid 2-4 sentence instruction for an image-generation model to repaint this exact photo, \
                 keeping its composition, in that medium\"}}",
                look = look_at(&cfg, &input),
                cat = crate::plugins::registry().catalogue_for_prompt()
            );
            stage(sh, &ctx, &format!("{} is studying your photo…", cfg.cli.name()));
            let text = run_text(&cfg, &prompt, &dir, &input, sh, &ctx)?;
            let report: DirectorReport = serde_json::from_str(extract_json(&text)?).context("parsing the director's JSON")?;
            Ok(AiOutput::Director(report))
        })
    }

    /// The Master Painter repaints the photo — Codex as a raster image, Claude as
    /// an SVG painting. With `director_brief`, the art director first expands
    /// the style into a photo-specific prompt (a "duet").
    pub fn repaint(&mut self, painter: &RoleCfg, director_brief: Option<&RoleCfg>, photo: Arc<Img>, medium: String, brief: String) -> Job {
        let ctx = self.ctx.clone();
        let painter = painter.clone();
        let director = director_brief.cloned();
        let engine = if director.is_some() { Engine::Duet } else { painter.engine() };
        let mut roles = vec![&painter];
        if let Some(d) = &director {
            roles.push(d);
        }
        let roles: Vec<RoleCfg> = roles.into_iter().cloned().collect();
        let role_refs: Vec<&RoleCfg> = roles.iter().collect();
        self.spawn(&format!("Repaint · {medium}"), engine, &role_refs, move |sh, dir| {
            let input = write_input(&photo, &dir, "photo.jpg")?;
            let mut style = brief.clone();
            if let Some(d) = &director {
                stage(sh, &ctx, &format!("{} is writing the brief…", d.cli.name()));
                let p = format!(
                    "{}. Write a single, rich prompt (3-5 sentences) for an image-generation model that will \
                     repaint THIS photograph as {}. Describe the subject and the exact composition so it can be compared \
                     side by side with the photo, then the materials, brushwork/marks, palette and light. \
                     Reply with the prompt text only; do not create files.",
                    look_at(d, &input),
                    brief
                );
                style = run_text(d, &p, &dir, &input, sh, &ctx)?.trim().to_string();
                log(sh, &ctx, format!("✍ Brief: {}", truncate(&style, 400)));
            }
            let who = painter.describe();
            let (img, subtitle) = match painter.cli {
                Cli::Codex => {
                    let out = dir.join("painting.png");
                    let prompt = format!(
                        "Use your image generation tool to repaint the attached photograph. Keep the exact composition, perspective \
                         and all major shapes so the painting can be compared side by side with the original photo. Do not add text, \
                         signatures or frames.\n\nStyle: {style}\n\nSave the final image as a PNG at {} (copy it there from wherever \
                         the tool stores it). Do not create any other files. When done, reply with just the path.",
                        out.display()
                    );
                    stage(sh, &ctx, "Codex is painting…");
                    let (thread_id, _) = run_codex(&prompt, &dir, &[input], &painter.model, true, sh, &ctx)?;
                    let found = if out.exists() { Some(out.clone()) } else { find_codex_image(thread_id.as_deref()) };
                    let path = found.ok_or_else(|| anyhow!("Codex finished but no image was produced"))?;
                    let img = image::open(&path).with_context(|| format!("opening {}", path.display()))?.to_rgb8();
                    (img, format!("{who} · image generation"))
                }
                Cli::Claude => {
                    let (vw, vh) = (1200, (1200.0 * photo.h as f32 / photo.w as f32).round() as u32);
                    let prompt = format!(
                        "{}. Repaint this photograph as a painting rendered entirely in SVG, in this medium: {style}\n\n\
                         Paint it rather than diagram it: build the image from 500-1500 overlapping, semi-transparent, \
                         brush-stroke-like shapes (curved paths with tapered ends), layered from background to foreground, \
                         with linearGradient/radialGradient for light. You may use SVG filters such as feTurbulence with \
                         feDisplacementMap or feGaussianBlur for paint texture. Keep the composition faithful (horizon, major \
                         masses, light source) so it can be compared with the photo.\n\
                         Output ONLY a complete standalone SVG document starting with <svg and ending with </svg>, with \
                         xmlns=\"http://www.w3.org/2000/svg\" and viewBox=\"0 0 {vw} {vh}\". No text elements, no external \
                         references, no images, no markdown fences, no commentary.",
                        look_at(&painter, &input)
                    );
                    stage(sh, &ctx, "Claude is painting in SVG…");
                    let text = run_claude(&prompt, &dir, &painter.model, sh, &ctx)?;
                    let svg = extract_svg(&text)?;
                    std::fs::write(dir.join("painting.svg"), svg)?;
                    stage(sh, &ctx, "Rasterising SVG…");
                    (rasterise_svg(svg, photo.w as u32, photo.h as u32)?, format!("{who} · SVG painting"))
                }
            };
            let subtitle = match &director {
                Some(d) => format!("Brief by {} → {subtitle}", d.describe()),
                None => subtitle,
            };
            let saved = photo_io::save_unique(&img, &photo_io::output_dir(), &format!("{}-{medium}", painter.cli.key()))?;
            Ok(AiOutput::Image { title: medium.clone(), subtitle, engine, img, prompt: style, saved })
        })
    }

    /// The director reinterprets the photo as hand-written SVG which we rasterise.
    pub fn vector(&mut self, director: &RoleCfg, photo: Arc<Img>, style: &'static VectorStyle) -> Job {
        let ctx = self.ctx.clone();
        let cfg = director.clone();
        self.spawn(&format!("Vector · {}", style.name), cfg.engine(), &[director], move |sh, dir| {
            let input = write_input(&photo, &dir, "photo.jpg")?;
            let (vw, vh) = (1200, (1200.0 * photo.h as f32 / photo.w as f32).round() as u32);
            let prompt = format!(
                "{}. Recreate this photograph as an original vector artwork in this style: {}.\n\
                 Keep the composition faithful (horizon, major masses, light source) so it can be compared with the photo.\n\
                 Output ONLY a complete standalone SVG document starting with <svg and ending with </svg>, \
                 with xmlns=\"http://www.w3.org/2000/svg\" and viewBox=\"0 0 {vw} {vh}\". Use 150-700 shapes. \
                 No text elements, no external references, no images, no markdown fences, no commentary, do not create files.",
                look_at(&cfg, &input),
                style.brief
            );
            stage(sh, &ctx, &format!("{} is cutting shapes…", cfg.cli.name()));
            let text = run_text(&cfg, &prompt, &dir, &input, sh, &ctx)?;
            let svg = extract_svg(&text)?;
            std::fs::write(dir.join("art.svg"), svg)?;
            stage(sh, &ctx, "Rasterising SVG…");
            let img = rasterise_svg(svg, photo.w as u32, photo.h as u32)?;
            let saved = photo_io::save_unique(&img, &photo_io::output_dir(), &format!("{}-{}", cfg.cli.key(), style.name))?;
            Ok(AiOutput::Image {
                title: style.name.to_string(),
                subtitle: format!("{} · vector reinterpretation", cfg.describe()),
                engine: cfg.engine(),
                img,
                prompt: style.brief.to_string(),
                saved,
            })
        })
    }

    /// The director writes a museum wall label for an artwork.
    pub fn placard(&mut self, director: &RoleCfg, artwork_id: u64, art: Arc<Img>, how: String) -> Job {
        let ctx = self.ctx.clone();
        let cfg = director.clone();
        self.spawn("Gallery placard", cfg.engine(), &[director], move |sh, dir| {
            let input = write_input(&art, &dir, "artwork.jpg")?;
            let prompt = format!(
                "{}. It is an artwork made from the user's own photograph ({how}). \
                 Write the museum wall label for it. Respond with ONLY a JSON object (do not create files): \
                 {{\"title\": \"evocative title, max 6 words\", \"medium\": \"plausible medium line, e.g. 'Oil on linen'\", \
                 \"year\": \"2026\", \"note\": \"a curatorial note of 45-70 words, specific to what is visible\"}}",
                look_at(&cfg, &input)
            );
            stage(sh, &ctx, &format!("{} is writing the wall label…", cfg.cli.name()));
            let text = run_text(&cfg, &prompt, &dir, &input, sh, &ctx)?;
            let placard: Placard = serde_json::from_str(extract_json(&text)?).context("parsing placard JSON")?;
            Ok(AiOutput::Placard { artwork_id, placard })
        })
    }
}

// ------------------------------------------------------------------ helpers

fn log(sh: &Arc<Mutex<JobShared>>, ctx: &egui::Context, line: String) {
    sh.lock().unwrap().log.push(line);
    ctx.request_repaint();
}

fn stage(sh: &Arc<Mutex<JobShared>>, ctx: &egui::Context, s: &str) {
    let mut g = sh.lock().unwrap();
    g.stage = s.to_string();
    g.log.push(format!("▸ {s}"));
    drop(g);
    ctx.request_repaint();
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n { s } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

fn write_input(img: &Img, dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let small = img.fit_long(1536).to_rgb8();
    small.save(&path).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn extract_json(text: &str) -> Result<&str> {
    let a = text.find('{').ok_or_else(|| anyhow!("no JSON in reply: {}", truncate(text, 200)))?;
    let b = text.rfind('}').ok_or_else(|| anyhow!("unterminated JSON in reply"))?;
    Ok(&text[a..=b])
}

fn extract_svg(text: &str) -> Result<&str> {
    let a = text.find("<svg").ok_or_else(|| anyhow!("no <svg> in reply: {}", truncate(text, 200)))?;
    let b = text.rfind("</svg>").ok_or_else(|| anyhow!("SVG was cut off"))?;
    Ok(&text[a..b + 6])
}

pub fn rasterise_svg(svg: &str, w: u32, h: u32) -> Result<image::RgbImage> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).context("parsing SVG")?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or_else(|| anyhow!("bad size"))?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    let s = tree.size();
    let tf = resvg::tiny_skia::Transform::from_scale(w as f32 / s.width(), h as f32 / s.height());
    resvg::render(&tree, tf, &mut pixmap.as_mut());
    let rgb: Vec<u8> = pixmap.data().as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    image::RgbImage::from_raw(w, h, rgb).ok_or_else(|| anyhow!("pixmap size"))
}

fn register_child(sh: &Arc<Mutex<JobShared>>, child: Child) -> Result<()> {
    let mut g = sh.lock().unwrap();
    if g.cancelled {
        let mut c = child;
        let _ = c.kill();
        bail!("cancelled");
    }
    g.child = Some(child);
    Ok(())
}

fn finish_child(sh: &Arc<Mutex<JobShared>>) -> Result<std::process::ExitStatus> {
    let child = sh.lock().unwrap().child.take();
    match child {
        Some(mut c) => Ok(c.wait()?),
        None => bail!("process vanished"),
    }
}

fn collect_stderr(child: &mut Child) -> std::thread::JoinHandle<String> {
    let mut err = child.stderr.take().expect("stderr piped");
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    })
}

/// `claude -p` with streamed JSON events; returns the final result text.
fn run_claude(prompt: &str, dir: &Path, model: &str, sh: &Arc<Mutex<JobShared>>, ctx: &egui::Context) -> Result<String> {
    let mut cmd = Command::new("claude");
    cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--no-session-persistence"]);
    if !model.is_empty() {
        cmd.args(["--model", model]);
    }
    let mut child = cmd
        .args(["--tools", "Read", "--allowedTools", "Read", "--add-dir"])
        .arg(dir)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting `claude` — is Claude Code installed and logged in?")?;
    {
        // The prompt goes over stdin: several claude flags are variadic and
        // would swallow a trailing positional argument.
        let mut stdin = child.stdin.take().expect("stdin piped");
        stdin.write_all(prompt.as_bytes())?;
    }
    let stdout = child.stdout.take().expect("stdout piped");
    let err = collect_stderr(&mut child);
    register_child(sh, child)?;

    let mut result: Option<(String, bool)> = None;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("assistant") => {
                for item in v["message"]["content"].as_array().into_iter().flatten() {
                    match item["type"].as_str() {
                        Some("tool_use") => {
                            let what = item["input"]["file_path"]
                                .as_str()
                                .map(|p| Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
                            log(sh, ctx, format!("👁 {} {}", item["name"].as_str().unwrap_or("tool"), what.unwrap_or_default()));
                        }
                        Some("text") => {
                            let t = item["text"].as_str().unwrap_or("");
                            if t.contains("<svg") {
                                log(sh, ctx, format!("✂ Composed an SVG of {} shapes", t.matches("/>").count()));
                            } else if t.trim_start().starts_with('{') {
                                log(sh, ctx, "📋 Returned structured notes".into());
                            } else if !t.trim().is_empty() {
                                log(sh, ctx, format!("💬 {}", truncate(t, 160)));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("result") => {
                let is_err = v["is_error"].as_bool().unwrap_or(false);
                result = Some((v["result"].as_str().unwrap_or("").to_string(), is_err));
            }
            _ => {}
        }
    }
    let status = finish_child(sh)?;
    let stderr = err.join().unwrap_or_default();
    if sh.lock().unwrap().cancelled {
        bail!("cancelled");
    }
    let key = RoleCfg::health_key_for(Cli::Claude, model);
    match result {
        Some((text, false)) => Ok(text),
        Some((text, true)) => match model_problem(&text) {
            Some(reason) => bail!("{MODEL_ERR}[{key}]: {reason}"),
            None => bail!("Claude reported an error: {}", truncate(&text, 300)),
        },
        None => match model_problem(&stderr) {
            Some(reason) => bail!("{MODEL_ERR}[{key}]: {reason}"),
            None => bail!("claude exited ({status}) without a result: {}", truncate(&stderr, 300)),
        },
    }
}

/// `codex exec --json`; returns the thread id (used to locate generated
/// images) and the final agent message.
fn run_codex(
    prompt: &str,
    dir: &Path,
    images: &[PathBuf],
    model: &str,
    image_gen: bool,
    sh: &Arc<Mutex<JobShared>>,
    ctx: &egui::Context,
) -> Result<(Option<String>, String)> {
    let mut cmd = Command::new("codex");
    cmd.args(["exec", "--skip-git-repo-check", "--json"]);
    if image_gen {
        cmd.args(["-s", "workspace-write", "--enable", "image_generation"]);
    } else {
        cmd.args(["-s", "read-only", "--disable", "image_generation"]);
    }
    if !model.is_empty() {
        cmd.args(["-m", model]);
    }
    cmd.arg("-C").arg(dir);
    for img in images {
        cmd.arg(format!("--image={}", img.display()));
    }
    let mut child = cmd
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting `codex` — is the Codex CLI installed and logged in?")?;
    {
        let mut stdin = child.stdin.take().expect("stdin piped");
        stdin.write_all(prompt.as_bytes())?;
    }
    let stdout = child.stdout.take().expect("stdout piped");
    let err = collect_stderr(&mut child);
    register_child(sh, child)?;

    let mut thread_id = None;
    let mut failure: Option<String> = None;
    let mut last_message = String::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("thread.started") => thread_id = v["thread_id"].as_str().map(String::from),
            Some("item.started") | Some("item.completed") => {
                let item = &v["item"];
                let done = v["type"] == "item.completed";
                match item["type"].as_str() {
                    Some("agent_message") if done => {
                        last_message = item["text"].as_str().unwrap_or("").to_string();
                        if last_message.contains("<svg") {
                            log(sh, ctx, format!("✂ Composed an SVG of {} shapes", last_message.matches("/>").count()));
                        } else if last_message.trim_start().starts_with('{') {
                            log(sh, ctx, "📋 Returned structured notes".into());
                        } else {
                            log(sh, ctx, format!("💬 {}", truncate(&last_message, 160)));
                        }
                    }
                    Some("command_execution") if !done => {
                        let c = item["command"].as_str().unwrap_or("");
                        let pretty = if c.contains("imagegen") || c.contains("SKILL.md") {
                            "Loading the image-generation skill".to_string()
                        } else if c.contains("generated_images") {
                            "Collecting the generated canvas".to_string()
                        } else {
                            truncate(c.trim_start_matches("/usr/bin/bash -lc "), 90)
                        };
                        log(sh, ctx, format!("⚙ {pretty}"));
                    }
                    Some("reasoning") if done => log(sh, ctx, "… thinking".into()),
                    Some(other) if !done && other != "agent_message" && other != "reasoning" => {
                        log(sh, ctx, format!("🎨 {}", other.replace('_', " ")));
                    }
                    _ => {}
                }
            }
            Some("turn.failed") | Some("error") => {
                let msg = v["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or("unknown error");
                failure = Some(msg.to_string());
            }
            _ => {}
        }
    }
    let status = finish_child(sh)?;
    let stderr = err.join().unwrap_or_default();
    if sh.lock().unwrap().cancelled {
        bail!("cancelled");
    }
    let key = RoleCfg::health_key_for(Cli::Codex, model);
    if let Some(f) = failure {
        match model_problem(&f) {
            Some(reason) => bail!("{MODEL_ERR}[{key}]: {reason}"),
            None => bail!("Codex: {f}"),
        }
    }
    if !status.success() {
        match model_problem(&stderr) {
            Some(reason) => bail!("{MODEL_ERR}[{key}]: {reason}"),
            None => bail!("codex exited with {status}: {}", truncate(&stderr, 300)),
        }
    }
    Ok((thread_id, last_message))
}

fn find_codex_image(thread_id: Option<&str>) -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".codex").join("generated_images").join(thread_id?);
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "png" || x == "jpg" || x == "webp"))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(image_gen: bool, models: &[&str]) -> CliStatus {
        CliStatus {
            found: true,
            version: "1".into(),
            image_gen,
            models: models.iter().map(|m| ModelInfo { id: m.to_string(), ..Default::default() }).collect(),
            default_model: None,
        }
    }

    #[test]
    fn roles_fall_back_to_preferred_installed_cli() {
        let prefs = Prefs::default();
        let (claude, codex) = (found(false, &["sonnet"]), found(true, &["gpt-x"]));
        assert_eq!(prefs.resolve(Role::Director, Some(&claude), Some(&codex)).unwrap().cli, Cli::Claude);
        assert_eq!(prefs.resolve(Role::Painter, Some(&claude), Some(&codex)).unwrap().cli, Cli::Codex);
        // Only Codex installed: it takes both roles.
        let missing = CliStatus::default();
        assert_eq!(prefs.resolve(Role::Director, Some(&missing), Some(&codex)).unwrap().cli, Cli::Codex);
        // Codex without image generation cannot paint; Claude (SVG) can.
        let codex_text_only = found(false, &[]);
        assert_eq!(prefs.resolve(Role::Painter, Some(&claude), Some(&codex_text_only)).unwrap().cli, Cli::Claude);
        assert!(prefs.resolve(Role::Painter, Some(&missing), Some(&codex_text_only)).is_none());
    }

    #[test]
    fn saved_choice_kept_only_while_valid() {
        let prefs = Prefs { director: Some(RoleCfg { cli: Cli::Codex, model: "gpt-x".into() }), ..Default::default() };
        let (claude, codex) = (found(false, &["sonnet"]), found(true, &["gpt-x"]));
        assert_eq!(prefs.resolve(Role::Director, Some(&claude), Some(&codex)), Some(RoleCfg { cli: Cli::Codex, model: "gpt-x".into() }));
        // The saved model disappeared from the catalogue: keep the CLI, use its default.
        let codex_new = found(true, &["gpt-y"]);
        assert_eq!(prefs.resolve(Role::Director, Some(&claude), Some(&codex_new)), Some(RoleCfg { cli: Cli::Codex, model: String::new() }));
        // Codex uninstalled: fall back to Claude.
        assert_eq!(prefs.resolve(Role::Director, Some(&claude), Some(&CliStatus::default())).unwrap().cli, Cli::Claude);
    }

    #[test]
    fn model_errors_are_recognised_and_round_trip() {
        let msg = "Fable 5.1 requires usage credits. Switch to another model, or manage usage credits.";
        let reason = model_problem(msg).unwrap();
        assert_eq!(reason, "Fable 5.1 requires usage credits");
        assert!(model_problem("rate limited, try again later").is_none());
        let err = format!("job: {MODEL_ERR}[claude:fable]: {reason}");
        assert_eq!(parse_model_error(&err), Some(("claude:fable".into(), reason)));
    }

    #[test]
    fn director_recipes_accept_typed_setting_values() {
        let json = r#"{"title": "T", "reading": "R", "recipes": [
            {"name": "A", "style": "pixel", "params": {"size": 9, "colors": 300}},
            {"name": "B", "style": "retro-print", "params": {"levels": 4, "invert": true, "ink": "sepia"}}
        ]}"#;
        let report: DirectorReport = serde_json::from_str(json).unwrap();
        use crate::plugins::Value;
        assert_eq!(report.recipes[1].params["invert"], Value::Bool(true));
        assert_eq!(report.recipes[1].params["ink"], Value::Text("sepia".into()));
        // Numbers are coerced into the style's ranges when applied.
        let registry = crate::plugins::Registry::builtin();
        let pixel = registry.get("pixel").unwrap();
        let p = crate::plugins::Params::sanitized(pixel.description(), &report.recipes[0].params);
        assert_eq!((p.get("size"), p.get("colors")), (9.0, 32.0));
    }

    #[test]
    fn extracts_json_and_svg_from_chatty_replies() {
        assert_eq!(extract_json("Sure! ```json\n{\"a\":1}\n```").unwrap(), "{\"a\":1}");
        assert_eq!(extract_svg("here: <svg x='1'><rect/></svg> done").unwrap(), "<svg x='1'><rect/></svg>");
        assert!(extract_svg("<svg><rect/>").is_err());
    }
}
