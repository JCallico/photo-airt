//! Starry Flow: one of the styles built into Photo·AIrt.
//!
//! Swirling, rhythmic strokes in the spirit of Van Gogh.

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
    ctx.progress(0.03);
    let base = blur_img(src, ctx.px(1.2));
    let fl = flow_field(src, ctx.px(p.get("swirl")));
    ctx.progress(0.25);
    let cell = ctx.px(p.get("width")).max(1.0);
    let jittered: Vec<Rgb> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let (cx, cy) = ((x / cell) as i32, (y / cell) as i32);
            let a = hash2(cx, cy, seed);
            let b = hash2(cx, cy, seed + 1);
            let mut hsv = rgb_to_hsv(base.px[i]);
            hsv[0] += (b - 0.5) * 0.05;
            hsv[2] *= 0.82 + 0.36 * a;
            hsv_to_rgb(hsv)
        })
        .collect();
    let noise = streak_noise(w, h, cell, seed + 2);
    if ctx.cancelled() {
        return None;
    }
    let len = ctx.px(p.get("length"));
    let smear = lic(&jittered, &fl, w, h, len);
    ctx.progress(0.6);
    let streak = lic(&noise, &fl, w, h, len);
    ctx.progress(0.85);
    let bands = p.get("bands").round();
    let vib = 1.0 + p.get("vibrance") * 0.9;
    let px: Vec<Rgb> = smear
        .par_iter()
        .zip(streak.par_iter())
        .map(|(&c, &s)| {
            let s = ((s - 0.5) * 3.0).clamp(-1.0, 1.0);
            let mut hsv = rgb_to_hsv(c);
            if bands >= 1.0 {
                let q = (hsv[2] * bands).round() / bands;
                hsv[2] = hsv[2] + (q - hsv[2]) * 0.55;
            }
            let c = saturate(hsv_to_rgb(hsv), vib);
            scale(c, 1.0 + 0.14 * s)
        })
        .collect();
    let out = Img { w, h, px };
    let lum = out.luma();
    let height: Vec<f32> = streak.iter().zip(lum.iter()).map(|(s, l)| (s - 0.5) * 2.0 * 0.7 + l * 0.3).collect();
    Some(apply_relief(&out, &height, p.get("impasto") * 0.8))
}
