//! Ink & Wash: one of the styles built into Photo·AIrt.
//!
//! Crisp ink lines over a soft sumi wash.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let seed = ctx.seed32();
    // Lines and wash come from an edge-preserving abstraction, so photo
    // texture (skin, foliage, grain) does not break up into ink speckle.
    let flow = flow_field(src, ctx.px(2.0));
    let l = akf(src, &flow, ctx.px(3.0).max(1.2), 8.0, 1.0, ctx, 0.02, 0.6)?.luma();
    let soft = p.get("softness");
    let e = xdog(&l, w, h, ctx.px(p.get("detail")).max(0.5), p.get("strength"), p.get("threshold"), 80.0 - 74.0 * soft);
    ctx.progress(0.75);
    // Stretch the wash tones between the 2nd and 98th percentiles so dim or
    // hazy photos still reach both pale paper and deep ink.
    let lb = blur(&l, w, h, ctx.px(3.0));
    let mut sorted = lb.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let (lo, hi) = (sorted[sorted.len() / 50], sorted[sorted.len() * 49 / 50]);
    let range = (hi - lo).max(0.1);
    let wash = p.get("wash");
    let sepia = p.get("sepia");
    let paper = mix([0.97, 0.97, 0.96], [0.96, 0.91, 0.8], sepia);
    let ink = mix([0.05, 0.05, 0.07], [0.2, 0.11, 0.05], sepia);
    let turb = ctx.px(90.0);
    let out = src.map_xy(|x, y, _| {
        let i = y * w + x;
        let t = ((lb[i] - lo) / range).clamp(0.0, 0.999) * 4.0;
        let f = t.floor();
        let q = (f + smoothstep(0.35, 0.65, t - f)) / 4.0;
        let tn = fbm(x as f32 / turb, y as f32 / turb, 4, seed) - 0.5;
        let wash_v = (1.0 - (1.0 - q) * wash * (1.0 + tn * 0.6)).clamp(0.0, 1.0);
        let base = mix(mix(paper, ink, 0.8), paper, wash_v);
        mix(ink, base, e[i])
    });
    Some(out)
}
