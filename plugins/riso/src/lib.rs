//! Risograph: one of the styles built into Photo·AIrt.
//!
//! Two fluorescent soy inks, grainy and slightly off-register.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};
use rayon::prelude::*;

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

const RISO: [(u32, u32); 5] =
    [(0xff48b0, 0x0078bf), (0x00838a, 0xff6c2f), (0xf15060, 0x3d5588), (0x00a95c, 0x765ba7), (0xffe800, 0x435060)];

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let pair = RISO[(p.get("inks").round() as usize).min(RISO.len() - 1)];
    let (i1, i2) = (hex(pair.0), hex(pair.1));
    let paper = [0.97, 0.95, 0.9];
    let dens = |c: Rgb| [-(c[0].max(0.03)).ln(), -(c[1].max(0.03)).ln(), -(c[2].max(0.03)).ln()];
    let (d1, d2) = (dens(i1), dens(i2));
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (a11, a12, a22) = (dot(d1, d1), dot(d1, d2), dot(d2, d2));
    let det = (a11 * a22 - a12 * a12).max(1e-6);
    let contrast = p.get("contrast");
    let base = blur_img(src, ctx.px(0.8));
    let cov: Vec<[f32; 2]> = base
        .px
        .par_iter()
        .map(|&c| {
            let c = [
                ((c[0] - 0.5) * contrast + 0.5).clamp(0.0, 1.0),
                ((c[1] - 0.5) * contrast + 0.5).clamp(0.0, 1.0),
                ((c[2] - 0.5) * contrast + 0.5).clamp(0.0, 1.0),
            ];
            let d = dens([c[0] / paper[0], c[1] / paper[1], c[2] / paper[2]]);
            let (b1, b2) = (dot(d1, d), dot(d2, d));
            let a = ((a22 * b1 - a12 * b2) / det).clamp(0.0, 1.0);
            let b = ((a11 * b2 - a12 * b1) / det).clamp(0.0, 1.0);
            [a, b]
        })
        .collect();
    ctx.progress(0.5);
    let grain = p.get("grain");
    let mis = ctx.px(p.get("misregister"));
    let seed = ctx.seed32();
    let gs = ctx.px(1.2).max(0.7);
    let out = src.map_xy(|x, y, _| {
        let a = cov[y * w + x][0];
        let bx = (x as f32 - mis * 0.8).clamp(0.0, (w - 1) as f32);
        let by = (y as f32 - mis * 0.6).clamp(0.0, (h - 1) as f32);
        let b = sample_bilinear(&cov, w, h, bx, by)[1];
        let screen = |v: f32, s: u32| {
            let t = value_noise(x as f32 / gs, y as f32 / gs, s);
            let hard = smoothstep(t - 0.18, t + 0.18, v);
            v + (hard - v) * grain
        };
        let (a, b) = (screen(a, seed), screen(b, seed + 1));
        mul(mul(paper, mix([1.0; 3], i1, a)), mix([1.0; 3], i2, b))
    });
    Some(out)
}
