//! Watercolour: one of the styles built into Photo·AIrt.
//!
//! Translucent washes, pooled pigment edges and a deckled paper border.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};
use rayon::prelude::*;

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

const PAPER: Rgb = [0.985, 0.97, 0.935];

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let seed = ctx.seed32();
    ctx.progress(0.02);
    let flow = flow_field(src, ctx.px(2.0));
    let abs = akf(src, &flow, ctx.px(p.get("abstraction")).max(1.2), 6.0, 1.0, ctx, 0.05, 0.45)?;

    let bleed = p.get("bleed");
    let soft = blur_img(&abs, ctx.px(7.0));
    let bleed_scale = ctx.px(70.0);
    let col = abs.map_xy(|x, y, c| {
        let m = fbm(x as f32 / bleed_scale, y as f32 / bleed_scale, 4, seed);
        mix(c, soft.at(x, y), smoothstep(0.42, 0.72, m) * bleed)
    });
    ctx.progress(0.55);

    let lum = col.luma();
    let lum_b = blur(&lum, w, h, ctx.px(2.5));
    let (edges, pigment, gran, light) = (p.get("edges"), p.get("pigment"), p.get("granulation"), p.get("lightness"));
    let (s_turb, s_g1, s_g2) = (ctx.px(110.0), ctx.px(1.3).max(0.8), ctx.px(4.0));
    let col = col.map_xy(|x, y, c| {
        let i = y * w + x;
        let (fx, fy) = (x as f32, y as f32);
        let e = ((lum[i] - lum_b[i]).abs() * 10.0).min(1.0);
        let d_edge = 1.0 + edges * e * 0.9;
        let d_turb = 1.0 + pigment * (fbm(fx / s_turb, fy / s_turb, 5, seed + 7) - 0.5) * 0.9;
        let g = value_noise(fx / s_g1, fy / s_g1, seed + 3) * 0.6 + value_noise(fx / s_g2, fy / s_g2, seed + 5) * 0.4;
        let d_gran = 1.0 + gran * (g - 0.5) * 0.9;
        let d = d_edge * d_turb * d_gran;
        let mut o = [0.0; 3];
        for k in 0..3 {
            // Bousseau et al.: C' = C - (C - C²)(d - 1)
            let v = c[k] - (c[k] - c[k] * c[k]) * (d - 1.0);
            o[k] = 1.0 - (1.0 - v.clamp(0.0, 1.0)) * (1.0 - 0.45 * light);
        }
        let tooth = 0.97 + 0.03 * value_noise(fx / 2.0, fy / 2.0, seed + 9);
        [o[0] * PAPER[0] * tooth, o[1] * PAPER[1] * tooth, o[2] * PAPER[2] * tooth]
    });
    ctx.progress(0.7);

    // Pencil underdrawing.
    let lines = p.get("lines");
    let col = if lines > 0.01 {
        let src_l = src.luma();
        let e = xdog(&src_l, w, h, ctx.px(1.1).max(0.6), 18.0, 0.82, 12.0);
        col.map_xy(|x, y, c| {
            let v = 1.0 - (1.0 - e[y * w + x]) * lines * 0.55;
            [c[0] * v, c[1] * (v * 0.98 + 0.02), c[2] * (v * 0.95 + 0.05)]
        })
    } else {
        col
    };
    ctx.progress(0.8);

    // Deckled border, pooled pigment at its edge, and splatter.
    let border = p.get("border");
    let s_border = ctx.px(50.0);
    let inset = (w.min(h) as f32) * 0.10;
    let mask: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let ed = (x.min(w - 1 - x).min(y).min(h - 1 - y)) as f32 / inset;
            let nz = fbm(x as f32 / s_border, y as f32 / s_border, 4, seed + 11) - 0.5;
            smoothstep(0.35, 0.8, ed + nz * 1.1)
        })
        .collect();
    let mut out = col.map_xy(|x, y, c| {
        let a = mask[y * w + x];
        let a2 = 1.0 - border * (1.0 - a);
        let pool = 1.0 - 0.3 * border * (a * (1.0 - a) * 4.0);
        mix(PAPER, scale(c, pool), a2)
    });
    if border > 0.01 {
        let mut rng = Rng::new(ctx.seed ^ 0xDEC0);
        let count = (420.0 * border) as usize;
        let mut placed = 0;
        let mut tries = 0;
        while placed < count && tries < count * 40 {
            tries += 1;
            let (x, y) = (rng.range(0.0, w as f32 - 1.0), rng.range(0.0, h as f32 - 1.0));
            let a = mask[y as usize * w + x as usize];
            if !(0.02..0.85).contains(&a) {
                continue;
            }
            let r = ctx.px(0.8 + 5.0 * rng.f32().powi(3)).max(0.7);
            let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
            let c = abs.sample(x + (cx - x) * 0.25, y + (cy - y) * 0.25);
            let c = mix(c, PAPER, 0.25);
            stamp_disc(&mut out, x, y, r, c, rng.range(0.45, 0.8));
            placed += 1;
        }
    }
    ctx.progress(0.98);
    Some(out)
}
