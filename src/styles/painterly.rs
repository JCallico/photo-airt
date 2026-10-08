use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use super::drawing::xdog;
use super::{Ctx, Params};
use crate::imaging::*;

// ------------------------------------------------------------------ AKF

/// Anisotropic Kuwahara filter with polynomial weighting functions
/// (Kyprianidis, Semmo, Kang & Döllner 2010). `p0..p1` is the progress span.
#[allow(clippy::too_many_arguments)]
pub(crate) fn akf(src: &Img, flow: &[Flow], radius: f32, q: f32, alpha: f32, ctx: &Ctx, p0: f32, p1: f32) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let zero_cross = 3.0 * std::f32::consts::PI / 8.0;
    let zeta = 2.0 / radius.max(1.0);
    let eta = (zeta + zero_cross.cos()) / zero_cross.sin().powi(2);
    let hardness = 8000.0f32;
    let done = AtomicUsize::new(0);
    let mut out = vec![[0.0f32; 3]; w * h];

    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if ctx.cancelled() {
            return;
        }
        for (x, o) in row.iter_mut().enumerate() {
            let f = flow[y * w + x];
            let (cos_p, sin_p, an) = (f[0], f[1], f[2]);
            let a = radius * ((alpha + an) / alpha).clamp(0.1, 2.0);
            let b = radius * (alpha / (alpha + an)).clamp(0.1, 2.0);
            let max_x = (a * a * cos_p * cos_p + b * b * sin_p * sin_p).sqrt().ceil() as isize;
            let max_y = (a * a * sin_p * sin_p + b * b * cos_p * cos_p).sqrt().ceil() as isize;
            let (sa, sb) = (0.5 / a, 0.5 / b);
            // Large kernels are sampled on a sparser lattice: same look, ~4x faster.
            let stride = if radius > 4.5 { 2 } else { 1 };
            let (max_x, max_y) = (max_x / stride * stride, max_y / stride * stride);

            let mut m = [[0.0f32; 4]; 8];
            let mut s = [[0.0f32; 3]; 8];
            for j in (-max_y..=max_y).step_by(stride as usize) {
                for i in (-max_x..=max_x).step_by(stride as usize) {
                    let (fi, fj) = (i as f32, j as f32);
                    let vx = (cos_p * fi + sin_p * fj) * sa;
                    let vy = (-sin_p * fi + cos_p * fj) * sb;
                    let vv = vx * vx + vy * vy;
                    if vv > 0.25 {
                        continue;
                    }
                    let c = src.get(x as isize + i, y as isize + j);
                    let mut wk = [0.0f32; 8];
                    let mut sum = 0.0;
                    let mut sector = |vx: f32, vy: f32, base: usize| {
                        let vxx = zeta - eta * vx * vx;
                        let vyy = zeta - eta * vy * vy;
                        let z = (vy + vxx).max(0.0);
                        wk[base] = z * z;
                        let z = (-vx + vyy).max(0.0);
                        wk[base + 2] = z * z;
                        let z = (-vy + vxx).max(0.0);
                        wk[base + 4] = z * z;
                        let z = (vx + vyy).max(0.0);
                        wk[base + 6] = z * z;
                        sum += wk[base] + wk[base + 2] + wk[base + 4] + wk[base + 6];
                    };
                    sector(vx, vy, 0);
                    let r2 = std::f32::consts::FRAC_1_SQRT_2;
                    sector(r2 * (vx - vy), r2 * (vx + vy), 1);
                    if sum <= 0.0 {
                        continue;
                    }
                    let g = (-3.125 * vv).exp() / sum;
                    let cc = [c[0] * c[0], c[1] * c[1], c[2] * c[2]];
                    for k in 0..8 {
                        let ww = wk[k] * g;
                        if ww == 0.0 {
                            continue;
                        }
                        m[k][0] += c[0] * ww;
                        m[k][1] += c[1] * ww;
                        m[k][2] += c[2] * ww;
                        m[k][3] += ww;
                        s[k][0] += cc[0] * ww;
                        s[k][1] += cc[1] * ww;
                        s[k][2] += cc[2] * ww;
                    }
                }
            }
            // Numerically stable version of w = 1 / (1 + (k σ²)^(q/2)).
            let mut xs = [f32::INFINITY; 8];
            let mut means = [[0.0f32; 3]; 8];
            let mut xmin = f32::INFINITY;
            for k in 0..8 {
                if m[k][3] <= 1e-12 {
                    continue;
                }
                let inv = 1.0 / m[k][3];
                let mean = [m[k][0] * inv, m[k][1] * inv, m[k][2] * inv];
                let var = (s[k][0] * inv - mean[0] * mean[0]).abs()
                    + (s[k][1] * inv - mean[1] * mean[1]).abs()
                    + (s[k][2] * inv - mean[2] * mean[2]).abs();
                means[k] = mean;
                xs[k] = 0.5 * q * (hardness * var).max(1e-12).ln();
                xmin = xmin.min(xs[k]);
            }
            let mut acc = [0.0f32; 4];
            for k in 0..8 {
                if !xs[k].is_finite() {
                    continue;
                }
                let wt = if xmin > 20.0 { (-(xs[k] - xmin)).exp() } else { 1.0 / (1.0 + xs[k].exp()) };
                acc[0] += means[k][0] * wt;
                acc[1] += means[k][1] * wt;
                acc[2] += means[k][2] * wt;
                acc[3] += wt;
            }
            *o = if acc[3] > 0.0 { [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3]] } else { src.at(x, y) };
        }
        let d = done.fetch_add(1, Ordering::Relaxed);
        if d.is_multiple_of(16) {
            ctx.progress(p0 + (p1 - p0) * d as f32 / h as f32);
        }
    });
    if ctx.cancelled() {
        return None;
    }
    Some(Img { w, h, px: out })
}

fn streak_noise(w: usize, h: usize, cell: f32, seed: u32) -> Vec<f32> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            hash2((x / cell) as i32, (y / cell) as i32, seed)
        })
        .collect()
}

// ------------------------------------------------------------------ oil

pub fn oil(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let radius = ctx.px(p.get("radius")).max(1.5);
    ctx.progress(0.02);
    let flow = flow_field(src, ctx.px(2.5).max(1.0));
    let out = akf(src, &flow, radius, p.get("sharpness"), p.get("anisotropy"), ctx, 0.05, 0.85)?;
    let sat = p.get("saturation");
    let out = out.map(|c| saturate(c, sat));

    // Brush ridges: noise smeared along the flow becomes the height map.
    let noise = streak_noise(w, h, (radius * 0.35).max(1.0), ctx.seed32());
    let streak = lic(&noise, &flow, w, h, radius * 1.6);
    let lum = out.luma();
    let height: Vec<f32> = lum.par_iter().zip(streak.par_iter()).map(|(l, s)| 0.55 * l + 0.45 * (s - 0.5) * 2.2).collect();
    let height = blur(&height, w, h, 0.7);
    ctx.progress(0.95);
    Some(apply_relief(&out, &height, p.get("impasto") * 0.9))
}

// ------------------------------------------------------------------ Hertzmann

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

pub fn brush(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
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

// ------------------------------------------------------------------ watercolour

const PAPER: Rgb = [0.985, 0.97, 0.935];

pub fn watercolor(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
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

// ------------------------------------------------------------------ starry flow

pub fn flow(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
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

// ------------------------------------------------------------------ pointillism

pub fn pointillism(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let dot = ctx.px(p.get("dot")).max(1.0);
    let spacing = (dot * 0.75 / p.get("density")).max(0.8);
    let jitter = p.get("jitter");
    let div = p.get("divisionism");
    let sat = p.get("saturation");
    let base = blur_img(src, spacing * 0.5);
    // Toned ground: gaps between dabs read as underpainting, not bare paper.
    let mut canvas = blur_img(&base, spacing * 2.0).map(|c| mix(c, [0.96, 0.94, 0.88], 0.3));
    let mut rng = Rng::new(ctx.seed);

    let dab = |rng: &mut Rng, x: f32, y: f32| -> Rgb {
        let c = saturate(base.sample(x, y), sat);
        let mut hsv = rgb_to_hsv(c);
        hsv[0] += rng.range(-jitter, jitter) * 0.3;
        hsv[2] *= 1.0 + rng.range(-jitter, jitter);
        if rng.f32() < div {
            // Divisionism: split the colour into neighbouring pure hues.
            hsv[0] += if rng.f32() < 0.5 { 0.07 } else { -0.07 };
            hsv[1] = (hsv[1] * 1.4).min(1.0);
        }
        clamp01(hsv_to_rgb(hsv))
    };

    let mut pts = Vec::new();
    let mut y = 0.0;
    while y < h as f32 {
        let mut x = 0.0;
        while x < w as f32 {
            pts.push([x + rng.range(0.0, spacing), y + rng.range(0.0, spacing)]);
            x += spacing;
        }
        y += spacing;
    }
    rng.shuffle(&mut pts);
    let total = pts.len();
    for (k, pt) in pts.into_iter().enumerate() {
        let c = dab(&mut rng, pt[0], pt[1]);
        let r = (dot * 0.5).max(spacing * 0.72) * rng.range(0.85, 1.2);
        stamp_disc(&mut canvas, pt[0], pt[1], r, c, 0.93);
        if k % 20000 == 0 {
            if ctx.cancelled() {
                return None;
            }
            ctx.progress(0.1 + 0.6 * k as f32 / total as f32);
        }
    }

    // Detail pass: smaller dabs where the photo has structure.
    let grad = gradient_mag(&src.luma(), w, h);
    let grad = blur(&grad, w, h, spacing * 0.5);
    let fine = spacing * 0.55;
    let mut pts = Vec::new();
    let mut y = 0.0;
    while y < h as f32 {
        let mut x = 0.0;
        while x < w as f32 {
            let (px, py) = (x + rng.range(0.0, fine), y + rng.range(0.0, fine));
            let g = grad[(py as usize).min(h - 1) * w + (px as usize).min(w - 1)];
            if g > 0.04 && rng.f32() < (g * 10.0).min(1.0) {
                pts.push([px, py]);
            }
            x += fine;
        }
        y += fine;
    }
    rng.shuffle(&mut pts);
    for pt in pts {
        let c = dab(&mut rng, pt[0], pt[1]);
        stamp_disc(&mut canvas, pt[0], pt[1], dot * 0.3 * rng.range(0.8, 1.2), c, 0.95);
    }
    ctx.progress(0.98);
    Some(canvas)
}
