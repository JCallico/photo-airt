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
    let l = src.luma();
    let soft = p.get("softness");
    let e = xdog(&l, w, h, ctx.px(p.get("detail")).max(0.5), p.get("strength"), p.get("threshold"), 80.0 - 74.0 * soft);
    ctx.progress(0.5);
    let lb = blur(&l, w, h, ctx.px(3.0));
    let wash = p.get("wash");
    let sepia = p.get("sepia");
    let paper = mix([0.97, 0.97, 0.96], [0.96, 0.91, 0.8], sepia);
    let ink = mix([0.05, 0.05, 0.07], [0.2, 0.11, 0.05], sepia);
    let turb = ctx.px(90.0);
    let out = src.map_xy(|x, y, _| {
        let i = y * w + x;
        let t = lb[i] * 4.0;
        let f = t.floor();
        let q = (f + smoothstep(0.35, 0.65, t - f)) / 4.0;
        let tn = fbm(x as f32 / turb, y as f32 / turb, 4, seed) - 0.5;
        let wash_v = (1.0 - (1.0 - q) * 1.1 * wash * (1.0 + tn * 0.6)).clamp(0.0, 1.0);
        let base = mix(mix(paper, ink, 0.55), paper, wash_v);
        mix(ink, base, e[i])
    });
    Some(out)
}
