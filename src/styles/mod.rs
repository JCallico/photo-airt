//! The algorithm catalogue. Every style is a pure function
//! `(&Img, &Params, &Ctx) -> Option<Img>` plus a parameter schema, which is
//! what lets the UI build sliders and lets Claude write recipes for it.

mod drawing;
mod graphic;
mod painterly;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::imaging::Img;

/// Size-like parameters are authored for a photo whose long side is this many
/// pixels; renders at other resolutions scale them so thumbnails, previews and
/// full-resolution exports all look alike.
pub const REFERENCE_LONG_SIDE: f32 = 1600.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Painterly,
    Drawing,
    Graphic,
    Print,
}

impl Family {
    pub const ALL: [Family; 4] = [Family::Painterly, Family::Drawing, Family::Graphic, Family::Print];
    pub fn label(self) -> &'static str {
        match self {
            Family::Painterly => "Painterly",
            Family::Drawing => "Drawing & Ink",
            Family::Graphic => "Geometric",
            Family::Print => "Print & Pixel",
        }
    }
}

pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// 1.0 → integer slider.
    pub step: f32,
}

const fn p(key: &'static str, label: &'static str, min: f32, max: f32, default: f32, step: f32) -> ParamSpec {
    ParamSpec { key, label, min, max, default, step }
}

pub struct StyleDef {
    pub id: &'static str,
    pub name: &'static str,
    pub family: Family,
    pub blurb: &'static str,
    pub technique: &'static str,
    pub params: &'static [ParamSpec],
    pub render: fn(&Img, &Params, &Ctx) -> Option<Img>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Params(pub BTreeMap<String, f32>);

impl Params {
    pub fn defaults(style: &StyleDef) -> Self {
        Params(style.params.iter().map(|s| (s.key.to_string(), s.default)).collect())
    }

    /// Merge AI-provided values over defaults, clamping into range.
    pub fn sanitized(style: &StyleDef, raw: &BTreeMap<String, f32>) -> Self {
        let mut out = Self::defaults(style);
        for spec in style.params {
            if let Some(v) = raw.get(spec.key)
                && v.is_finite()
            {
                let mut v = v.clamp(spec.min, spec.max);
                if spec.step >= 1.0 {
                    v = v.round();
                }
                out.0.insert(spec.key.to_string(), v);
            }
        }
        out
    }

    pub fn get(&self, key: &str) -> f32 {
        *self.0.get(key).unwrap_or_else(|| panic!("unknown param {key}"))
    }
}

/// Render context: resolution scaling, seed, progress and cancellation.
#[derive(Clone)]
pub struct Ctx {
    pub scale: f32,
    pub seed: u64,
    pub progress: Arc<AtomicU32>,
    pub cancel: Arc<AtomicBool>,
}

impl Ctx {
    pub fn for_image(img: &Img, seed: u64) -> Self {
        Self {
            scale: img.long_side() as f32 / REFERENCE_LONG_SIDE,
            seed,
            progress: Arc::new(AtomicU32::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    /// Scale a reference-resolution pixel size to this render.
    pub fn px(&self, v: f32) -> f32 {
        (v * self.scale).max(0.5)
    }
    pub fn progress(&self, f: f32) {
        self.progress.store((f.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn seed32(&self) -> u32 {
        (self.seed ^ (self.seed >> 32)) as u32
    }
}

pub static STYLES: &[StyleDef] = &[
    StyleDef {
        id: "oil",
        name: "Oil on Canvas",
        family: Family::Painterly,
        blurb: "Thick, sculpted paint that follows the forms of your photo.",
        technique: "Anisotropic Kuwahara filter with polynomial sector weights (Kyprianidis 2010) steered by a smoothed structure tensor, then relit as impasto.",
        params: &[
            p("radius", "Brush radius", 2.0, 14.0, 6.0, 0.5),
            p("sharpness", "Sharpness", 1.0, 18.0, 8.0, 0.5),
            p("anisotropy", "Stroke elongation", 0.2, 3.0, 1.0, 0.05),
            p("impasto", "Impasto relief", 0.0, 1.0, 0.45, 0.01),
            p("saturation", "Pigment saturation", 0.6, 1.8, 1.15, 0.01),
        ],
        render: painterly::oil,
    },
    StyleDef {
        id: "brush",
        name: "Impressionist Strokes",
        family: Family::Painterly,
        blurb: "Thousands of curved bristle strokes laid down coarse to fine.",
        technique: "Hertzmann's multi-layer curved brush-stroke painter (SIGGRAPH '98) with structure-tensor stroke guidance, bristle texture and an impasto height map.",
        params: &[
            p("brush", "Largest brush", 4.0, 40.0, 16.0, 1.0),
            p("layers", "Layers", 1.0, 4.0, 3.0, 1.0),
            p("threshold", "Detail threshold", 0.02, 0.3, 0.09, 0.005),
            p("length", "Stroke length", 2.0, 24.0, 10.0, 1.0),
            p("curvature", "Curvature", 0.0, 1.0, 0.6, 0.01),
            p("jitter", "Colour jitter", 0.0, 0.25, 0.06, 0.005),
            p("bristles", "Bristle texture", 0.0, 1.0, 0.5, 0.01),
            p("impasto", "Impasto relief", 0.0, 1.0, 0.5, 0.01),
        ],
        render: painterly::brush,
    },
    StyleDef {
        id: "watercolor",
        name: "Watercolour",
        family: Family::Painterly,
        blurb: "Translucent washes, pooled pigment edges and a deckled paper border.",
        technique: "Abstraction + wet-in-wet bleeding, Bousseau's pigment-density model for edge darkening, turbulence and granulation, XDoG pencil lines and a noise-carved vignette with splatter.",
        params: &[
            p("abstraction", "Wash simplification", 1.0, 10.0, 4.0, 0.5),
            p("bleed", "Wet bleeding", 0.0, 1.0, 0.5, 0.01),
            p("edges", "Edge darkening", 0.0, 1.0, 0.6, 0.01),
            p("pigment", "Pigment turbulence", 0.0, 1.0, 0.5, 0.01),
            p("granulation", "Granulation", 0.0, 1.0, 0.45, 0.01),
            p("lines", "Pencil underdrawing", 0.0, 1.0, 0.35, 0.01),
            p("border", "Deckled border", 0.0, 1.0, 0.7, 0.01),
            p("lightness", "Transparency", 0.0, 1.0, 0.35, 0.01),
        ],
        render: painterly::watercolor,
    },
    StyleDef {
        id: "flow",
        name: "Starry Flow",
        family: Family::Painterly,
        blurb: "Swirling, rhythmic strokes in the spirit of Van Gogh.",
        technique: "Line integral convolution of colour-jittered stroke noise along a heavily smoothed edge-tangent flow, with tone banding and relief lighting.",
        params: &[
            p("length", "Stroke length", 4.0, 40.0, 18.0, 1.0),
            p("width", "Stroke width", 1.0, 8.0, 3.0, 0.5),
            p("swirl", "Swirl smoothness", 2.0, 24.0, 9.0, 0.5),
            p("vibrance", "Vibrance", 0.0, 1.0, 0.55, 0.01),
            p("bands", "Tone bands (0 = off)", 0.0, 12.0, 7.0, 1.0),
            p("impasto", "Impasto relief", 0.0, 1.0, 0.55, 0.01),
        ],
        render: painterly::flow,
    },
    StyleDef {
        id: "pointillism",
        name: "Pointillism",
        family: Family::Painterly,
        blurb: "Seurat-style dabs of pure colour that mix in the eye.",
        technique: "Jittered blue-noise dot placement with divisionist colour splitting and a second fine pass where the photo has detail.",
        params: &[
            p("dot", "Dot size", 2.0, 16.0, 6.0, 0.5),
            p("density", "Density", 0.5, 2.0, 1.0, 0.05),
            p("jitter", "Colour jitter", 0.0, 0.3, 0.1, 0.005),
            p("divisionism", "Divisionism", 0.0, 0.6, 0.25, 0.01),
            p("saturation", "Saturation", 0.8, 2.0, 1.35, 0.01),
        ],
        render: painterly::pointillism,
    },
    StyleDef {
        id: "pencil",
        name: "Graphite Sketch",
        family: Family::Drawing,
        blurb: "Pencil on paper with hatching that follows the shapes.",
        technique: "Colour-dodge sketch (grey ÷ blurred inverse) combined with flow-aligned LIC hatching and paper tooth.",
        params: &[
            p("weight", "Line weight", 1.0, 12.0, 4.0, 0.5),
            p("darkness", "Darkness", 0.0, 1.0, 0.55, 0.01),
            p("hatching", "Hatching", 0.0, 1.0, 0.6, 0.01),
            p("color", "Colour pencil", 0.0, 1.0, 0.0, 0.01),
            p("paper", "Paper tooth", 0.0, 1.0, 0.5, 0.01),
        ],
        render: drawing::pencil,
    },
    StyleDef {
        id: "ink",
        name: "Ink & Wash",
        family: Family::Drawing,
        blurb: "Crisp ink lines over a soft sumi wash.",
        technique: "Winnemöller's eXtended Difference-of-Gaussians (XDoG) with a soft tanh threshold, over a posterised tonal wash.",
        params: &[
            p("detail", "Line detail (σ)", 0.4, 4.0, 1.4, 0.05),
            p("strength", "Edge emphasis", 5.0, 60.0, 16.0, 0.5),
            p("threshold", "Threshold", 0.2, 1.2, 0.62, 0.01),
            p("softness", "Softness", 0.0, 1.0, 0.4, 0.01),
            p("wash", "Tonal wash", 0.0, 1.0, 0.6, 0.01),
            p("sepia", "Sepia paper", 0.0, 1.0, 0.5, 0.01),
        ],
        render: drawing::ink,
    },
    StyleDef {
        id: "toon",
        name: "Cel Animation",
        family: Family::Drawing,
        blurb: "Flat shading bands and bold outlines, like a hand-inked frame.",
        technique: "Edge-preserving abstraction, HSV value quantisation with soft steps and XDoG outlines.",
        params: &[
            p("smoothing", "Smoothing", 1.0, 8.0, 4.0, 0.5),
            p("levels", "Shade levels", 2.0, 10.0, 5.0, 1.0),
            p("outline", "Outline", 0.0, 1.0, 0.7, 0.01),
            p("saturation", "Saturation", 0.6, 2.0, 1.35, 0.01),
        ],
        render: drawing::toon,
    },
    StyleDef {
        id: "stained_glass",
        name: "Stained Glass",
        family: Family::Graphic,
        blurb: "Leaded glass panes glowing with the colours of your scene.",
        technique: "Detail-adaptive Voronoi tessellation by jump flooding, per-cell colour averaging, chamfer distance-field lead came and glass texture.",
        params: &[
            p("cell", "Pane size", 8.0, 90.0, 30.0, 1.0),
            p("adaptivity", "Detail adaptivity", 0.0, 1.0, 0.6, 0.01),
            p("lead", "Lead width", 0.5, 6.0, 2.2, 0.1),
            p("glow", "Glow", 0.0, 1.0, 0.45, 0.01),
            p("texture", "Glass texture", 0.0, 1.0, 0.5, 0.01),
        ],
        render: graphic::stained_glass,
    },
    StyleDef {
        id: "lowpoly",
        name: "Low Poly",
        family: Family::Graphic,
        blurb: "Faceted triangles that crystallise the scene.",
        technique: "Edge-weighted point sampling, Delaunay triangulation and per-facet colour with directional facet lighting.",
        params: &[
            p("points", "Triangles", 200.0, 8000.0, 1800.0, 50.0),
            p("focus", "Edge focus", 0.0, 1.0, 0.7, 0.01),
            p("light", "Facet light", 0.0, 1.0, 0.3, 0.01),
            p("outline", "Wireframe", 0.0, 1.0, 0.0, 0.01),
        ],
        render: graphic::lowpoly,
    },
    StyleDef {
        id: "popart",
        name: "Pop Art Quad",
        family: Family::Graphic,
        blurb: "Four screen-printed panels in clashing pop palettes.",
        technique: "Tone posterisation mapped through four hand-picked palettes, with Ben-Day dots and ink outlines.",
        params: &[
            p("levels", "Tone levels", 2.0, 5.0, 4.0, 1.0),
            p("outline", "Outline", 0.0, 1.0, 0.5, 0.01),
            p("dots", "Ben-Day dots", 0.0, 1.0, 0.35, 0.01),
        ],
        render: graphic::popart,
    },
    StyleDef {
        id: "halftone",
        name: "CMYK Halftone",
        family: Family::Print,
        blurb: "Rosette dot screens of a vintage four-colour press.",
        technique: "Per-channel rotated amplitude-modulated screens (15°/75°/0°/45°), subtractive ink compositing and plate misregistration.",
        params: &[
            p("cell", "Screen size", 3.0, 24.0, 8.0, 0.5),
            p("angle", "Screen rotation", 0.0, 90.0, 0.0, 1.0),
            p("density", "Ink density", 0.5, 1.5, 0.85, 0.01),
            p("misregister", "Misregistration", 0.0, 4.0, 0.8, 0.1),
        ],
        render: graphic::halftone,
    },
    StyleDef {
        id: "riso",
        name: "Risograph",
        family: Family::Print,
        blurb: "Two fluorescent soy inks, grainy and slightly off-register.",
        technique: "Least-squares optical-density separation into two ink layers, stochastic grain screening and drum misregistration.",
        params: &[
            p("inks", "Ink pair", 0.0, 4.0, 0.0, 1.0),
            p("grain", "Grain", 0.0, 1.0, 0.5, 0.01),
            p("misregister", "Misregistration", 0.0, 10.0, 3.0, 0.1),
            p("contrast", "Contrast", 0.5, 2.0, 1.2, 0.01),
        ],
        render: graphic::riso,
    },
    StyleDef {
        id: "pixel",
        name: "Pixel Art",
        family: Family::Print,
        blurb: "A limited-palette sprite of your photo, 16-bit style.",
        technique: "Area downsampling, k-means++ palette extraction, ordered Bayer dithering and sprite outlines.",
        params: &[
            p("size", "Pixel size", 2.0, 24.0, 7.0, 1.0),
            p("colors", "Palette colours", 2.0, 32.0, 14.0, 1.0),
            p("dither", "Dithering", 0.0, 1.0, 0.45, 0.01),
            p("outline", "Outlines", 0.0, 1.0, 0.3, 0.01),
            p("saturation", "Saturation", 0.6, 2.0, 1.2, 0.01),
        ],
        render: graphic::pixel,
    },
];

pub fn style_index(id: &str) -> Option<usize> {
    STYLES.iter().position(|s| s.id == id)
}

/// A compact catalogue for AI prompts.
pub fn catalogue_for_prompt() -> String {
    let mut s = String::new();
    for st in STYLES {
        s.push_str(&format!("- \"{}\" ({}): {}\n  params: ", st.id, st.name, st.blurb));
        let ps: Vec<String> =
            st.params.iter().map(|p| format!("{} [{}..{}, default {}] = {}", p.key, p.min, p.max, p.default, p.label)).collect();
        s.push_str(&ps.join("; "));
        s.push('\n');
    }
    s
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
    fn every_style_renders_finite_pixels_at_input_size() {
        let src = gradient(96, 64);
        for st in STYLES {
            let ctx = Ctx::for_image(&src, 3);
            let out = (st.render)(&src, &Params::defaults(st), &ctx).unwrap_or_else(|| panic!("{} returned None", st.id));
            assert_eq!((out.w, out.h), (src.w, src.h), "{} changed the size", st.id);
            assert!(out.px.iter().flatten().all(|v| v.is_finite()), "{} produced non-finite pixels", st.id);
        }
    }

    #[test]
    fn style_ids_are_unique_and_defaults_in_range() {
        let mut ids: Vec<_> = STYLES.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), STYLES.len());
        for st in STYLES {
            for p in st.params {
                assert!((p.min..=p.max).contains(&p.default), "{}.{} default out of range", st.id, p.key);
            }
        }
    }

    #[test]
    fn sanitized_params_clamp_and_round() {
        let st = &STYLES[style_index("pixel").unwrap()];
        let raw = [("size".to_string(), 999.0), ("colors".to_string(), 7.6), ("bogus".to_string(), 1.0)].into_iter().collect();
        let p = Params::sanitized(st, &raw);
        assert_eq!(p.get("size"), 24.0);
        assert_eq!(p.get("colors"), 8.0);
        assert!(!p.0.contains_key("bogus"));
    }

    #[test]
    fn cancelled_render_returns_none() {
        let src = gradient(64, 48);
        let ctx = Ctx::for_image(&src, 1);
        ctx.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let st = &STYLES[style_index("oil").unwrap()];
        assert!((st.render)(&src, &Params::defaults(st), &ctx).is_none());
    }
}
