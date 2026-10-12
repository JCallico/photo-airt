//! Ink & Toon: one of the styles built into Photo·AIrt.
//!
//! Clean shapes and bold outlines, from a sepia ink wash to a cel-shaded frame.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

/// Soft quantisation into `q` bands: flat regions with short ramps between.
fn bands(v: f32, q: f32) -> f32 {
    let t = v * q;
    let f = t.floor();
    ((f + smoothstep(0.42, 0.58, t - f)) / q).min(1.0)
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let (q, colour, outline, sepia) = (p.get("levels").round().max(2.0), p.get("colour"), p.get("outline"), p.get("sepia"));
    let sat = 1.0 + 0.35 * colour;

    // Edge-preserving abstraction: everything below is drawn from clean shapes.
    let flow = flow_field(src, ctx.px(2.0));
    let abs = akf(src, &flow, ctx.px(p.get("smoothing")).max(1.2), 8.0, 1.0, ctx, 0.02, 0.75)?;
    let l = abs.luma();
    let e = xdog(&l, w, h, ctx.px(p.get("detail")).max(0.6), 22.0, 0.8, 28.0);

    // The ink wash uses the tones stretched between the 2nd and 98th
    // percentiles, so dim photos still reach both paper and deep ink.
    let lb = blur(&l, w, h, ctx.px(2.0));
    let mut sorted = lb.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let (lo, hi) = (sorted[sorted.len() / 50], sorted[sorted.len() * 49 / 50]);
    let range = (hi - lo).max(0.1);
    let paper = mix([0.97, 0.97, 0.96], [0.96, 0.91, 0.8], sepia);
    let ink = mix([0.05, 0.05, 0.07], [0.2, 0.11, 0.05], sepia);
    let turb = ctx.px(90.0);
    let seed = ctx.seed32();

    let out = abs.map_xy(|x, y, c| {
        let i = y * w + x;
        let wash = if colour < 1.0 {
            let t = bands(((lb[i] - lo) / range).clamp(0.0, 0.999), q);
            let tn = fbm(x as f32 / turb, y as f32 / turb, 4, seed) - 0.5;
            mix(ink, paper, (t * (1.0 + tn * 0.25)).clamp(0.0, 1.0))
        } else {
            paper
        };
        let cel = if colour > 0.0 {
            let mut hsv = rgb_to_hsv(c);
            hsv[2] = bands(hsv[2], q);
            saturate(hsv_to_rgb(hsv), sat)
        } else {
            paper
        };
        let base = mix(wash, cel, colour);
        mix(mix(ink, [0.0; 3], colour), base, 1.0 - (1.0 - e[i]) * outline)
    });
    Some(out)
}
