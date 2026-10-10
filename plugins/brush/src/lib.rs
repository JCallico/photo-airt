//! Impressionist Strokes: one of the styles built into Photo·AIrt.
//!
//! Thousands of curved bristle strokes laid down coarse to fine.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};
use rayon::prelude::*;

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

struct Stroke {
    pts: Vec<[f32; 2]>,
    color: Rgb,
    r: f32,
    seed: u32,
}

#[allow(clippy::too_many_arguments)]
fn make_stroke(
    x0: usize,
    y0: usize,
    r: f32,
    reference: &Img,
    canvas: &Img,
    painted: &[bool],
    flow: &[Flow],
    max_len: usize,
    fc: f32,
) -> Stroke {
    let (w, h) = (reference.w, reference.h);
    let color = reference.at(x0, y0);
    let mut pts = vec![[x0 as f32, y0 as f32]];
    let (mut x, mut y) = (x0 as f32, y0 as f32);
    let mut last = [0.0f32, 0.0];
    let min_len = 2;
    for i in 1..=max_len {
        let xi = (x.round() as usize).min(w - 1);
        let yi = (y.round() as usize).min(h - 1);
        let idx = yi * w + xi;
        if i > min_len {
            let rc = reference.px[idx];
            let canvas_d = if painted[idx] { dist2(rc, canvas.px[idx]) } else { f32::INFINITY };
            if canvas_d < dist2(rc, color) {
                break;
            }
        }
        let f = flow[idx];
        if f[3] < 1e-4 && i > min_len {
            break;
        }
        let mut d = [f[0], f[1]];
        if i > 1 {
            if d[0] * last[0] + d[1] * last[1] < 0.0 {
                d = [-d[0], -d[1]];
            }
            d = [fc * d[0] + (1.0 - fc) * last[0], fc * d[1] + (1.0 - fc) * last[1]];
            let n = d[0].hypot(d[1]);
            if n < 1e-6 {
                break;
            }
            d = [d[0] / n, d[1] / n];
        }
        x += r * d[0];
        y += r * d[1];
        if x < 0.0 || y < 0.0 || x > (w - 1) as f32 || y > (h - 1) as f32 {
            break;
        }
        last = d;
        pts.push([x, y]);
    }
    Stroke { pts, color, r, seed: (x0 as u32).wrapping_mul(73_856_093) ^ (y0 as u32).wrapping_mul(19_349_663) }
}

/// Paint one stroke, clipped to rows `y_lo..y_hi` of a band whose buffers
/// start at row `y_lo`. Bands paint every stroke in the same order, so the
/// result is identical to a serial render.
#[allow(clippy::too_many_arguments)]
fn paint_stroke_band(
    canvas: &mut [Rgb],
    painted: &mut [bool],
    height: &mut [f32],
    w: usize,
    y_lo: usize,
    y_hi: usize,
    s: &Stroke,
    bristles: f32,
    max_r: f32,
) {
    let r = s.r;
    let spacing = (r * 0.5).max(0.5);
    let bristle_count = (r * 0.9).max(3.0);
    let thickness = r.sqrt() / max_r.sqrt();
    // Bristle profile across the brush, sampled once per stroke.
    const N: usize = 32;
    let profile: [f32; N] =
        std::array::from_fn(|k| value_noise(k as f32 / (N - 1) as f32 * 2.0 * bristle_count, s.seed as f32 * 0.0137, s.seed));
    let mut stamp = |cx: f32, cy: f32, n: [f32; 2]| {
        let x0 = (cx - r - 1.0).floor().max(0.0) as usize;
        let x1 = ((cx + r + 1.0).ceil() as usize).min(w - 1);
        let y0 = ((cy - r - 1.0).floor().max(0.0) as usize).max(y_lo);
        let y1 = ((cy + r + 1.0).ceil() as usize).min(y_hi - 1);
        if y0 > y1 {
            return;
        }
        for y in y0..=y1 {
            let row = (y - y_lo) * w;
            for x in x0..=x1 {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let d2 = dx * dx + dy * dy;
                if d2 > (r + 0.5) * (r + 0.5) {
                    continue;
                }
                let dist = d2.sqrt();
                let cov = (r + 0.5 - dist).min(1.0);
                let sp = ((dx * n[0] + dy * n[1]) / r).clamp(-1.0, 1.0);
                let br = profile[((sp + 1.0) * 0.5 * (N - 1) as f32) as usize];
                let k = 1.0 + bristles * 0.28 * (br - 0.5);
                let i = row + x;
                canvas[i] = mix(canvas[i], [s.color[0] * k, s.color[1] * k, s.color[2] * k], cov);
                painted[i] = true;
                let prof = (1.0 - d2 / (r * r)).max(0.0).sqrt();
                let hh = (0.55 * prof + 0.45 * br * bristles) * thickness;
                height[i] += (hh - height[i]) * cov;
            }
        }
    };
    if s.pts.len() == 1 {
        stamp(s.pts[0][0], s.pts[0][1], [0.0, 1.0]);
        return;
    }
    for seg in s.pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy).max(1e-3);
        let n = [-dy / len, dx / len];
        let steps = (len / spacing).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            stamp(a[0] + dx * t, a[1] + dy * t, n);
        }
    }
}

fn stroke_y_range(s: &Stroke) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in &s.pts {
        lo = lo.min(p[1]);
        hi = hi.max(p[1]);
    }
    (lo - s.r - 1.0, hi + s.r + 1.0)
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let max_r = ctx.px(p.get("brush")).max(1.5);
    let layers = p.get("layers").round().max(1.0) as i32;
    let threshold = p.get("threshold");
    let max_len = p.get("length").round() as usize;
    let fc = p.get("curvature");
    let jitter = p.get("jitter");
    let bristles = p.get("bristles");

    ctx.progress(0.02);
    let flow = flow_field(src, ctx.px(4.0));
    let mut canvas = Img::new(w, h, [0.93, 0.9, 0.84]);
    let mut painted = vec![false; w * h];
    let mut height = vec![0.0f32; w * h];
    let mut rng = Rng::new(ctx.seed);

    for l in 0..layers {
        if ctx.cancelled() {
            return None;
        }
        let r = (max_r / 2f32.powi(l)).max(1.2);
        let reference = blur_img(src, r * 0.5);
        let grid = r.round().max(1.0) as usize;
        let cells: Vec<(usize, usize)> = (0..h).step_by(grid).flat_map(|y| (0..w).step_by(grid).map(move |x| (x, y))).collect();
        let mut strokes: Vec<Stroke> = cells
            .par_iter()
            .filter_map(|&(cx, cy)| {
                let (mut sum, mut n, mut maxd, mut best) = (0.0f32, 0usize, -1.0f32, (cx, cy));
                for y in cy..(cy + grid).min(h) {
                    for x in cx..(cx + grid).min(w) {
                        let i = y * w + x;
                        let d = if painted[i] { dist2(canvas.px[i], reference.px[i]).sqrt() } else { 1.0 };
                        sum += d;
                        n += 1;
                        if d > maxd {
                            maxd = d;
                            best = (x, y);
                        }
                    }
                }
                (sum / n as f32 > threshold).then(|| make_stroke(best.0, best.1, r, &reference, &canvas, &painted, &flow, max_len, fc))
            })
            .collect();
        rng.shuffle(&mut strokes);
        for s in strokes.iter_mut() {
            let j = |rng: &mut Rng| 1.0 + rng.range(-jitter, jitter);
            let b = j(&mut rng);
            s.color = clamp01([s.color[0] * b * j(&mut rng), s.color[1] * b * j(&mut rng), s.color[2] * b * j(&mut rng)]);
        }
        if ctx.cancelled() {
            return None;
        }
        let ranges: Vec<(f32, f32)> = strokes.iter().map(stroke_y_range).collect();
        let band_h = 32usize;
        canvas
            .px
            .par_chunks_mut(w * band_h)
            .zip(painted.par_chunks_mut(w * band_h))
            .zip(height.par_chunks_mut(w * band_h))
            .enumerate()
            .for_each(|(bi, ((cv, pt), ht))| {
                let y_lo = bi * band_h;
                let y_hi = (y_lo + band_h).min(h);
                for (s, &(lo, hi)) in strokes.iter().zip(ranges.iter()) {
                    if hi >= y_lo as f32 && lo < y_hi as f32 {
                        paint_stroke_band(cv, pt, ht, w, y_lo, y_hi, s, bristles, max_r);
                    }
                }
            });
        ctx.progress(0.05 + 0.9 * (l + 1) as f32 / layers as f32);
    }
    let height = blur(&height, w, h, 0.6);
    Some(apply_relief(&canvas, &height, p.get("impasto") * 0.8))
}
