//! Graphite Sketch: one of the styles built into Photo·AIrt.
//!
//! Pencil on paper with hatching that follows the shapes.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};
use rayon::prelude::*;

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
    let inv: Vec<f32> = l.iter().map(|v| 1.0 - v).collect();
    let b = blur(&inv, w, h, ctx.px(p.get("weight")));
    ctx.progress(0.2);
    let darkness = p.get("darkness");
    let dodge: Vec<f32> =
        l.par_iter().zip(b.par_iter()).map(|(&v, &bb)| (v / (1.0 - bb).max(0.02)).min(1.0).powf(1.0 + 2.5 * darkness)).collect();

    let flow = flow_field(src, ctx.px(4.0));
    let cell = ctx.px(1.0).max(1.0);
    let noise: Vec<f32> =
        (0..w * h).into_par_iter().map(|i| hash2(((i % w) as f32 / cell) as i32, ((i / w) as f32 / cell) as i32, seed)).collect();
    if ctx.cancelled() {
        return None;
    }
    let hatch = lic(&noise, &flow, w, h, ctx.px(12.0));
    ctx.progress(0.7);
    let l_soft = blur(&l, w, h, ctx.px(3.0));
    let (hatching, color, paper) = (p.get("hatching"), p.get("color"), p.get("paper"));
    let out = src.map_xy(|x, y, c| {
        let i = y * w + x;
        let hs = ((hatch[i] - 0.5) * 3.0 + 0.5).clamp(0.0, 1.0);
        let mut v = dodge[i] - hatching * (1.0 - l_soft[i]) * 0.55 * hs;
        let tooth = value_noise(x as f32 / 1.4, y as f32 / 1.4, seed + 5);
        v = 1.0 - (1.0 - v.clamp(0.0, 1.0)) * (1.0 - paper * 0.45 * tooth);
        let li = l[i].max(0.05);
        let chroma = [(c[0] / li).min(2.0), (c[1] / li).min(2.0), (c[2] / li).min(2.0)];
        let tint = mix([1.0, 1.0, 1.0], chroma, color);
        let g = [v * 0.97, v * 0.97, v];
        let shade = mix(g, mul(g, tint), (1.0 - v * 0.5) * color.min(1.0) + color * 0.3);
        mul(clamp01(shade), [0.985, 0.975, 0.95])
    });
    ctx.progress(0.98);
    Some(out)
}
