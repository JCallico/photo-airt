//! Scratchboard: one of the styles built into Photo·AIrt.
//!
//! Light scratched out of night-dark ink, with a glowing sun and stars.

use std::f32::consts::TAU;

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};
use rayon::prelude::*;

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

/// Ink-to-paper ramp: navy-black ink through neutral grey to warm cream.
const RAMP: [Rgb; 5] = [[0.039, 0.057, 0.075], [0.229, 0.239, 0.241], [0.447, 0.443, 0.417], [0.672, 0.645, 0.583], [0.92, 0.853, 0.741]];

fn ramp(t: f32, tint: f32) -> Rgb {
    let x = t.clamp(0.0, 1.0) * (RAMP.len() - 1) as f32;
    let k = (x as usize).min(RAMP.len() - 2);
    let c = mix(RAMP[k], RAMP[k + 1], x - k as f32);
    let grey = 0.04 + 0.9 * t.clamp(0.0, 1.0);
    mix([grey; 3], c, tint)
}

/// Night tone: warm light (the sun, lit cloud rims, skin) stays bright while
/// saturated blues (sky, water) fall towards black, like black-and-white film
/// behind a deep red filter. `wide` is the luminance averaged over a large
/// neighbourhood; subtracting it adds local contrast. The weights are a
/// least-squares fit to a reference engraving of a sunset photo.
fn night_tone(c: Rgb, wide: f32) -> f32 {
    let l = luma(c);
    let sat = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
    1.353 * c[0] + 2.228 * c[0] * c[0] - 2.722 * l * l - 1.259 * sat - 0.258 * wide + 0.099
}

/// The brightest blurred spot, if it is bright enough to read as a light
/// source: its position and strength.
fn light_source(tone: &[f32], w: usize, h: usize, sigma: f32) -> Option<(f32, f32, f32)> {
    let b = blur(tone, w, h, sigma);
    let (i, &v) = b.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    (v > 0.6).then(|| ((i % w) as f32, (i / w) as f32, ((v - 0.6) / 0.3).min(1.0)))
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let seed = ctx.seed32();
    let (night, detail, texture, lines, rays, tint) =
        (p.get("night"), p.get("detail"), p.get("texture"), p.get("lines"), p.get("rays"), p.get("tint"));

    // Clean shapes first, so marks describe forms rather than photo noise.
    ctx.progress(0.02);
    let flow = flow_field(src, ctx.px(2.0).max(1.0));
    let abs = akf(src, &flow, ctx.px(2.5).max(1.2), 8.0, 1.0, ctx, 0.03, 0.4)?;

    // Tone: luminance pulled towards the night tone, plus local contrast.
    let lum = abs.luma();
    let wide = blur(&lum, w, h, ctx.px(50.0));
    let near = blur(&lum, w, h, ctx.px(12.0));
    let tone: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            // Night falls on blue sky and water; other colours keep most of
            // their own brightness, so foliage, skin and stone stay readable.
            let c = abs.px[i];
            let blue = smoothstep(-0.04, 0.06, c[2] - c[0].max(c[1]) * 0.9);
            let dark = night_tone(c, wide[i]).min(lum[i] * 1.15);
            let t = lum[i] + (dark - lum[i]) * night * (0.25 + 0.75 * blue);
            (t + detail * (lum[i] - near[i]) * 1.5).clamp(0.0, 1.0)
        })
        .collect();
    // Photos without a bright sky or sun would otherwise sink into the ink:
    // make sure the highlights reach paper and the median stays readable.
    let mut sorted = tone.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let (median, top) = (sorted[sorted.len() / 2].max(1e-3), sorted[sorted.len() * 99 / 100].max(1e-3));
    let gain = (0.9 / top).clamp(1.0, 2.5);
    let gamma = if median * gain < 0.22 { ((0.22f32).ln() / (median * gain).min(0.99).ln()).clamp(0.45, 1.0) } else { 1.0 };
    let tone: Vec<f32> = tone.par_iter().map(|&t| (t * gain).min(1.0).powf(gamma)).collect();
    ctx.progress(0.5);
    if ctx.cancelled() {
        return None;
    }

    // Marks: fine grain plus short strokes along the form, used as a
    // threshold so every mark is hard-edged and its density carries tone.
    let cell = ctx.px(p.get("scratch")).max(0.6);
    let grain: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32 / cell, (i / w) as f32 / cell);
            value_noise(x, y, seed) * 0.75 + value_noise(x * 2.3, y * 2.3, seed + 1) * 0.25
        })
        .collect();
    let strokes = lic(&grain, &flow, w, h, ctx.px(6.0));
    let marks: Vec<f32> = grain.iter().zip(&strokes).map(|(g, s)| 0.5 + ((g * 0.35 + s * 0.65) - 0.5) * 3.0).collect();
    ctx.progress(0.7);

    // Contours, the light source and flat dark areas for stars.
    let e = xdog(&lum, w, h, ctx.px(1.2).max(0.6), 20.0, 0.82, 20.0);
    let grad = blur(&gradient_mag(&tone, w, h), w, h, ctx.px(2.0));
    // Open sky: smooth, bright, blue regions connected to the top edge. Only
    // there do stars appear (not on water, foliage or dark backgrounds).
    let smooth_blue: Vec<bool> = (0..w * h)
        .map(|i| {
            let c = abs.px[i];
            c[2] >= c[0] && c[2] >= c[1] * 0.95 && lum[i] > 0.3 && grad[i] < 0.03
        })
        .collect();
    let mut sky = vec![false; w * h];
    for x in 0..w {
        for y in 0..h {
            let i = y * w + x;
            if !smooth_blue[i] || (y > 0 && !sky[i - w]) {
                break;
            }
            sky[i] = true;
        }
    }
    // Where the picture has structure: flat sky and water stay calm.
    let structure = blur(&gradient_mag(&lum, w, h), w, h, ctx.px(4.0));
    let mut sorted = structure.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let busy = sorted[sorted.len() * 9 / 10].max(1e-4);
    let sun = light_source(&tone, w, h, ctx.px(12.0));
    let (ray_len, star_cell) = (ctx.px(330.0), ctx.px(9.0).max(2.0));
    ctx.progress(0.85);

    let out = src.map_xy(|x, y, _| {
        let i = y * w + x;
        let mut t = tone[i];
        if let Some((sx, sy, strength)) = sun {
            let (dx, dy) = (x as f32 - sx, y as f32 - sy);
            let d = dx.hypot(dy);
            let a = dy.atan2(dx) / TAU + 0.5;
            // Broken rays: angular streaks that flicker along their length.
            let streak = value_noise(a * 160.0, 0.5, seed + 3).powi(4) * 0.7 + value_noise(a * 37.0, 1.5, seed + 4).powi(2) * 0.3;
            let flicker = value_noise(a * 160.0, d / ctx.px(14.0), seed + 5);
            t += rays * strength * (streak * flicker * (-d / ray_len).exp() * 0.7 + (-d / (ray_len * 0.12)).exp() * 0.5);
        }
        // Scratches show most in the mid-tones; deep ink and highlights stay calmer.
        let hard = smoothstep(marks[i] - 0.08, marks[i] + 0.08, t);
        let band = (-((t - 0.38) / 0.24).powi(2)).exp() * 0.75 + 0.25;
        let busy = (structure[i] / busy).min(1.0);
        let mut v = t + (hard - t) * texture * band * (0.3 + 0.7 * busy);
        // Stars: sparse specks where the open sky turned dark.
        let (cx, cy) = ((x as f32 / star_cell) as i32, (y as f32 / star_cell) as i32);
        let r = hash2(cx, cy, seed + 7);
        if r > 1.0 - 0.12 * rays && sky[i] && tone[i] < 0.3 && grad[i] < 0.015 {
            let (ox, oy) = (hash2(cx, cy, seed + 8) * star_cell, hash2(cx, cy, seed + 9) * star_cell);
            let size = ctx.px(0.6 + 2.4 * ((r - 1.0 + 0.12 * rays) / (0.12 * rays)).powi(3));
            let dd = (x as f32 - cx as f32 * star_cell - ox).hypot(y as f32 - cy as f32 * star_cell - oy);
            v = v.max(smoothstep(size + 0.6, size - 0.4, dd) * 0.85);
        }
        v *= 1.0 - (1.0 - e[i]) * lines;
        ramp(v, tint)
    });
    ctx.progress(0.98);
    Some(out)
}
