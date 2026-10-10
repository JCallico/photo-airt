//! The plug-in system for painting styles (BL-001).
//!
//! Every style — the fourteen built-in ones compiled into this executable and
//! the external plug-ins — is a [`StylePlugin`]: it *describes itself*
//! (identity plus the settings the Atelier panel shows, decision D4) and
//! renders an image. The rest of the app only ever talks to the
//! [`Registry`], so it cannot tell built-in and external styles apart.
//!
//! Every style's description comes from its `plugin.toml`
//! ([`photo_airt_sdk::Manifest`]): read from disk for external plug-ins,
//! embedded at compile time for built-in ones. The schema ([`Description`],
//! [`photo_airt_sdk::Setting`], [`Params`], [`Ctx`]) comes from
//! `photo-airt-sdk`, so both kinds share it.

mod builtin;
pub mod external;

use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

pub use photo_airt_sdk::{Ctx, Description, Params, SettingKind, Value};

use crate::imaging::Img;

/// One entry of the style catalogue given to the Art Director.
fn catalogue_entry(desc: &Description) -> String {
    let settings: Vec<String> = desc
        .settings
        .iter()
        .map(|s| match (&s.kind, &s.default) {
            (SettingKind::Number { min, max, .. }, Value::Number(d)) => {
                format!("{} [{}..{}, default {}] = {}", s.key, min, max, d, s.label)
            }
            (SettingKind::Toggle, Value::Bool(d)) => format!("{} [true|false, default {}] = {}", s.key, d, s.label),
            (SettingKind::Choice { options }, Value::Text(d)) => {
                let opts: Vec<String> = options.iter().map(|o| format!("\"{}\"", o.value)).collect();
                format!("{} [one of {}, default \"{}\"] = {}", s.key, opts.join("|"), d, s.label)
            }
            _ => format!("{} = {}", s.key, s.label),
        })
        .collect();
    format!("- \"{}\" ({}): {}\n  params: {}\n", desc.id, desc.name, desc.blurb, settings.join("; "))
}

// ------------------------------------------------------------------ plug-in trait

/// Why a render produced no image.
#[derive(Clone, Debug, PartialEq)]
pub enum RenderError {
    /// The host cancelled it (a newer render superseded it, or the user left).
    Cancelled,
    /// The style failed; the message is shown to the user.
    Failed(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Cancelled => write!(f, "cancelled"),
            RenderError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// A painting style. Implementations must be deterministic for a given seed,
/// return an image the same size as the input, scale size-like settings with
/// [`Ctx::px`], report progress and honour cancellation.
pub trait StylePlugin: Send + Sync {
    fn description(&self) -> &Description;

    /// Render an image the same size as `img`.
    fn render(&self, img: &Img, params: &Params, ctx: &Ctx) -> Result<Img, RenderError>;

    fn id(&self) -> &str {
        &self.description().id
    }

    /// The folder an external plug-in was loaded from; `None` for built-ins.
    fn folder(&self) -> Option<&Path> {
        None
    }
}

// ------------------------------------------------------------------ registry

/// All styles available in this session, in gallery order.
pub struct Registry {
    plugins: Vec<Arc<dyn StylePlugin>>,
}

impl Registry {
    /// The styles compiled into this executable (decision D3).
    pub fn builtin() -> Self {
        let plugins = builtin::all();
        for p in &plugins {
            debug_assert!(
                p.description().validate().is_ok(),
                "built-in style breaks the plug-in contract: {:?}",
                p.description().validate()
            );
        }
        Self { plugins }
    }

    /// Built-in styles followed by the ready external plug-ins of a scan.
    pub fn with_external(scan: &external::Scan) -> Self {
        let mut reg = Self::builtin();
        reg.plugins.extend(scan.plugins());
        reg
    }

    /// Ids of the styles compiled into the executable.
    pub fn builtin_ids() -> Vec<String> {
        builtin::all().iter().map(|p| p.id().to_string()).collect()
    }

    pub fn all(&self) -> &[Arc<dyn StylePlugin>] {
        &self.plugins
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn StylePlugin>> {
        self.plugins.iter().find(|p| p.id() == id)
    }

    /// The default style shown when a photo is first opened.
    pub fn first_id(&self) -> String {
        self.plugins.first().map(|p| p.id().to_string()).unwrap_or_default()
    }

    /// Gallery families in first-appearance order.
    pub fn families(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for p in &self.plugins {
            let f = &p.description().family;
            if !out.contains(f) {
                out.push(f.clone());
            }
        }
        out
    }

    /// Compact catalogue of every style and its settings, for AI prompts.
    pub fn catalogue_for_prompt(&self) -> String {
        self.plugins.iter().map(|p| catalogue_entry(p.description())).collect()
    }
}

static REGISTRY: OnceLock<RwLock<Arc<Registry>>> = OnceLock::new();

fn slot() -> &'static RwLock<Arc<Registry>> {
    REGISTRY.get_or_init(|| RwLock::new(Arc::new(Registry::builtin())))
}

/// The current registry. Cheap to call; hold the `Arc` for the duration of
/// an operation rather than calling repeatedly.
pub fn registry() -> Arc<Registry> {
    slot().read().expect("registry lock").clone()
}

/// Replace the registry (after scanning for external plug-ins).
pub fn install(registry: Registry) {
    *slot().write().expect("registry lock") = Arc::new(registry);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Img {
        let mut img = Img::new(w, h, [0.0; 3]);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                img.px[y * w + x] = [u, v, ((u * 12.0).sin() * 0.5 + 0.5) * (1.0 - v)];
            }
        }
        img
    }

    #[test]
    fn builtin_registry_is_valid_and_complete() {
        let reg = Registry::builtin();
        assert_eq!(reg.all().len(), 14);
        let mut ids: Vec<_> = reg.all().iter().map(|p| p.id().to_string()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 14, "ids must be unique");
        for p in reg.all() {
            p.description().validate().unwrap_or_else(|e| panic!("{e}"));
        }
        assert_eq!(reg.families(), ["Painterly", "Drawing & Ink", "Geometric", "Print & Pixel"]);
        assert_eq!(reg.first_id(), "oil");
    }

    #[test]
    fn every_style_renders_finite_pixels_at_input_size() {
        let src = gradient(96, 64);
        for p in Registry::builtin().all() {
            let ctx = Ctx::for_image(&src, 3);
            let out = p.render(&src, &Params::defaults(p.description()), &ctx).unwrap_or_else(|e| panic!("{} failed: {e}", p.id()));
            assert_eq!((out.w, out.h), (src.w, src.h), "{} changed the size", p.id());
            assert!(out.px.iter().flatten().all(|v| v.is_finite()), "{} produced non-finite pixels", p.id());
        }
    }

    #[test]
    fn every_style_is_deterministic_for_a_seed() {
        let src = gradient(80, 60);
        for p in Registry::builtin().all() {
            let params = Params::defaults(p.description());
            let a = p.render(&src, &params, &Ctx::for_image(&src, 11)).unwrap();
            let b = p.render(&src, &params, &Ctx::for_image(&src, 11)).unwrap();
            assert!(a.px == b.px, "{} is not deterministic for a fixed seed", p.id());
        }
    }

    #[test]
    fn cancelled_render_returns_none() {
        let src = gradient(64, 48);
        let ctx = Ctx::for_image(&src, 1);
        ctx.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let reg = Registry::builtin();
        let oil = reg.get("oil").unwrap();
        assert_eq!(oil.render(&src, &Params::defaults(oil.description()), &ctx).err(), Some(RenderError::Cancelled));
    }
}
