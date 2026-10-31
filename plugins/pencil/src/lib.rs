//! Graphite Sketch: one of the styles built into Photo·AIrt.
//!
//! Pencil on paper: direction-classified strokes over hatching that matches
//! the photo's tones (Lu, Xu & Jia, "Combining Sketch and Tone for Pencil
//! Drawing Production", NPAR 2012).

use std::f32::consts::PI;

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};
use rayon::prelude::*;

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

const DIRECTIONS: usize = 8;

/// Convolve with a centred line segment of `len` pixels at `angle`, sampled
/// bilinearly (a box filter along the line).
fn line_conv(src: &[f32], w: usize, h: usize, angle: f32, len: f32) -> Vec<f32> {
    let (dx, dy) = (angle.cos(), angle.sin());
    let steps = len.round().max(2.0) as i32;
    let half = steps as f32 / 2.0;
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let mut acc = 0.0;
            for s in 0..=steps {
                let t = s as f32 - half;
                acc += sample_bilinear(src, w, h, x + dx * t, y + dy * t);
            }
            acc / (steps + 1) as f32
        })
        .collect()
}

/// Lu et al.'s stroke map: each gradient pixel votes for the direction whose
/// line response is strongest, and is redrawn as a short line in that
/// direction. Returns 1 for paper, lower for graphite.
fn stroke_map(lum: &[f32], w: usize, h: usize, len: f32, ctx: &Ctx) -> Option<Vec<f32>> {
    // Gradients below the noise floor (skin, sky, paper grain) draw nothing.
    let g = gradient_mag(lum, w, h);
    let mut sorted = g.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let floor = sorted[sorted.len() * 7 / 10];
    let g: Vec<f32> = g.iter().map(|&v| (v - floor).max(0.0)).collect();
    let angles: Vec<f32> = (0..DIRECTIONS).map(|k| k as f32 * PI / DIRECTIONS as f32).collect();
    let responses: Vec<Vec<f32>> = angles.iter().map(|&a| line_conv(&g, w, h, a, len)).collect();
    ctx.progress(0.3);
    if ctx.cancelled() {
        return None;
    }
    let best: Vec<usize> = (0..w * h)
        .into_par_iter()
        .map(|i| (0..DIRECTIONS).max_by(|&a, &b| responses[a][i].total_cmp(&responses[b][i])).unwrap_or(0))
        .collect();
    let mut sum = vec![0.0f32; w * h];
    for (k, &a) in angles.iter().enumerate() {
        let c: Vec<f32> = g.iter().zip(&best).map(|(&v, &b)| if b == k { v } else { 0.0 }).collect();
        let drawn = line_conv(&c, w, h, a, len);
        sum.iter_mut().zip(drawn).for_each(|(s, d)| *s += d);
    }
    ctx.progress(0.5);
    let mut sorted = sum.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let top = sorted[sorted.len() * 995 / 1000].max(1e-6);
    Some(sum.iter().map(|&s| 1.0 - (s / top).min(1.0)).collect())
}

/// Lu et al.'s pencil tone model: a mix of a bright layer (Laplacian near
/// white), a mid-tone layer (uniform) and a dark layer (Gaussian), in 256 bins.
fn target_cdf() -> [f32; 256] {
    let (w_bright, w_mid, w_dark) = (52.0, 37.0, 11.0);
    let mut pdf = [0.0f32; 256];
    for (v, p) in pdf.iter_mut().enumerate() {
        let v = v as f32;
        let bright = (-(255.0 - v) / 9.0).exp() / 9.0;
        let mid = if (105.0..=225.0).contains(&v) { 1.0 / 120.0 } else { 0.0 };
        let dark = (-(v - 90.0).powi(2) / (2.0 * 11.0 * 11.0)).exp() / (11.0 * (2.0 * PI).sqrt());
        *p = w_bright * bright + w_mid * mid + w_dark * dark;
    }
    let total: f32 = pdf.iter().sum();
    let mut cdf = [0.0f32; 256];
    let mut acc = 0.0;
    for (c, p) in cdf.iter_mut().zip(pdf) {
        acc += p / total;
        *c = acc;
    }
    cdf
}

/// Histogram-match `lum` to the pencil tone model.
fn pencil_tone(lum: &[f32]) -> Vec<f32> {
    let mut hist = [0usize; 256];
    for &v in lum {
        hist[(v.clamp(0.0, 1.0) * 255.0) as usize] += 1;
    }
    let target = target_cdf();
    let mut map = [0.0f32; 256];
    let mut acc = 0usize;
    for (v, &n) in hist.iter().enumerate() {
        acc += n;
        let q = acc as f32 / lum.len() as f32;
        map[v] = target.iter().position(|&c| c >= q).unwrap_or(255) as f32 / 255.0;
    }
    lum.iter().map(|&v| map[(v.clamp(0.0, 1.0) * 255.0) as usize]).collect()
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let seed = ctx.seed32();
    let (darkness, hatching, color, paper) = (p.get("darkness"), p.get("hatching"), p.get("color"), p.get("paper"));
    ctx.progress(0.02);
    let lum = blur(&src.luma(), w, h, ctx.px(1.2));

    // Outline strokes.
    let strokes = stroke_map(&lum, w, h, ctx.px(p.get("stroke")).max(3.0), ctx)?;
    if ctx.recording() {
        ctx.snapshot(&src.map_xy(|x, y, _| {
            let v = strokes[y * w + x].clamp(0.0, 1.0);
            mul([v * 0.97, v * 0.97, v], [0.985, 0.975, 0.95])
        }));
    }

    // Tone, then the pencil texture rendered to that tone: H^β ≈ J, with β
    // from the locally averaged texture so the hatching keeps its grain.
    let tone = blur(&pencil_tone(&lum), w, h, ctx.px(1.5));
    // Straight diagonal hatching, as in a pencil texture: thin parallel
    // strokes broken into dashes of varying pressure.
    let spacing = ctx.px(2.2).max(1.2);
    let dash = ctx.px(28.0).max(4.0);
    let grain: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let across = (x - y * 0.5) / spacing;
            let along = (x * 0.5 + y) / dash;
            let line = across.floor() as i32;
            let offset = hash2(line, 7, seed) * 4.0;
            let pressure = value_noise(along + offset, line as f32 * 0.37, seed + 1);
            let profile = 1.0 - (across.rem_euclid(1.0) - 0.5).abs() * 2.0;
            pressure * profile.powf(0.7) + value_noise(x / 1.3, y / 1.3, seed + 2) * 0.25
        })
        .collect();
    let hatch = blur(&grain, w, h, ctx.px(0.5));
    if ctx.cancelled() {
        return None;
    }
    let (hlo, hhi) = {
        let mut s = hatch.clone();
        s.sort_unstable_by(f32::total_cmp);
        (s[s.len() / 50], s[s.len() * 49 / 50].max(s[s.len() / 50] + 1e-3))
    };
    let texture: Vec<f32> = hatch.iter().map(|&v| 0.55 + 0.4 * ((v - hlo) / (hhi - hlo)).clamp(0.0, 1.0)).collect();
    let tex_mean = blur(&texture, w, h, ctx.px(6.0));
    ctx.progress(0.8);

    let gamma = 0.6 + 2.4 * darkness;
    let out = src.map_xy(|x, y, c| {
        let i = y * w + x;
        let j = tone[i].clamp(0.02, 1.0);
        let beta = j.ln() / tex_mean[i].min(0.999).ln();
        let t = j + (texture[i].powf(beta) - j) * hatching;
        let mut v = (t * strokes[i].powf(gamma)).clamp(0.0, 1.0);
        let tooth = value_noise(x as f32 / 1.4, y as f32 / 1.4, seed + 5);
        v = 1.0 - (1.0 - v) * (1.0 - paper * 0.35 * tooth);
        let li = luma(c).max(0.05);
        let chroma = [(c[0] / li).min(2.0), (c[1] / li).min(2.0), (c[2] / li).min(2.0)];
        let g = [v * 0.97, v * 0.97, v];
        let shade = mix(g, mul(g, chroma), color * (1.0 - v * 0.4));
        mul(clamp01(shade), [0.985, 0.975, 0.95])
    });
    ctx.progress(0.98);
    Some(out)
}
