use std::f32::consts::PI;

use rayon::prelude::*;

use super::drawing::xdog;
use super::{Ctx, Params};
use crate::imaging::*;

// ------------------------------------------------------------------ stained glass

pub fn stained_glass(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let seed = ctx.seed32();
    let cell = ctx.px(p.get("cell")).max(4.0);
    let adapt = p.get("adaptivity");
    let mut rng = Rng::new(ctx.seed);

    let grad = gradient_mag(&src.luma(), w, h);
    let grad = blur(&grad, w, h, cell * 0.5);
    let mut seeds: Vec<[f32; 2]> = Vec::new();
    let mut y = 0.0;
    while y < h as f32 {
        let mut x = 0.0;
        while x < w as f32 {
            let (cx, cy) = ((x + cell * 0.5).min(w as f32 - 1.0), (y + cell * 0.5).min(h as f32 - 1.0));
            let detail = (grad[cy as usize * w + cx as usize] * 9.0).min(1.0);
            if rng.f32() < adapt * detail {
                for (qx, qy) in [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)] {
                    seeds.push([x + (qx + rng.range(0.05, 0.45)) * cell, y + (qy + rng.range(0.05, 0.45)) * cell]);
                }
            } else {
                seeds.push([x + rng.range(0.1, 0.9) * cell, y + rng.range(0.1, 0.9) * cell]);
            }
            x += cell;
        }
        y += cell;
    }
    seeds.retain(|s| s[0] < w as f32 && s[1] < h as f32);
    ctx.progress(0.1);

    // Jump flooding Voronoi.
    let mut ids = vec![u32::MAX; w * h];
    for (k, s) in seeds.iter().enumerate() {
        ids[s[1] as usize * w + s[0] as usize] = k as u32;
    }
    let mut step = w.max(h).next_power_of_two() / 2;
    let mut passes = vec![];
    while step >= 1 {
        passes.push(step);
        step /= 2;
    }
    passes.push(1);
    let npass = passes.len();
    for (pi, step) in passes.into_iter().enumerate() {
        if ctx.cancelled() {
            return None;
        }
        let prev = ids.clone();
        let si = step as isize;
        ids.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut best = prev[y * w + x];
                let d = |id: u32| {
                    let s = seeds[id as usize];
                    (s[0] - x as f32).powi(2) + (s[1] - y as f32).powi(2)
                };
                let mut bd = if best == u32::MAX { f32::INFINITY } else { d(best) };
                for dy in -1..=1isize {
                    for dx in -1..=1isize {
                        let (nx, ny) = (x as isize + dx * si, y as isize + dy * si);
                        if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                            continue;
                        }
                        let c = prev[ny as usize * w + nx as usize];
                        if c != u32::MAX && c != best {
                            let cd = d(c);
                            if cd < bd {
                                bd = cd;
                                best = c;
                            }
                        }
                    }
                }
                *o = best;
            }
        });
        ctx.progress(0.1 + 0.4 * (pi + 1) as f32 / npass as f32);
    }

    let mut sums = vec![[0.0f32; 4]; seeds.len()];
    for (i, &id) in ids.iter().enumerate() {
        let c = src.px[i];
        let s = &mut sums[id as usize];
        s[0] += c[0];
        s[1] += c[1];
        s[2] += c[2];
        s[3] += 1.0;
    }
    let colors: Vec<Rgb> = sums
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let n = s[3].max(1.0);
            let jit = 0.92 + 0.16 * hash2(k as i32, 7, seed);
            saturate(scale([s[0] / n, s[1] / n, s[2] / n], jit), 1.45)
        })
        .collect();

    let mask: Vec<bool> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            (x + 1 < w && ids[i] != ids[i + 1]) || (y + 1 < h && ids[i] != ids[i + w])
        })
        .collect();
    let dt = distance_transform(&mask, w, h);
    ctx.progress(0.7);

    let (texture, glow) = (p.get("texture"), p.get("glow"));
    let (tex_s, d1, d2) = (ctx.px(14.0), ctx.px(4.0), ctx.px(12.0));
    let glass = src.map_xy(|x, y, _| {
        let i = y * w + x;
        let mut c = colors[ids[i] as usize];
        let t = fbm(x as f32 / tex_s, y as f32 / tex_s, 3, seed + 3);
        let streak = value_noise(x as f32 / (tex_s * 0.25), y as f32 / (tex_s * 3.0), seed + 4);
        c = scale(c, 1.0 + texture * ((t - 0.5) * 0.4 + (streak - 0.5) * 0.12));
        c = scale(c, 1.18 - 0.16 * (-dt[i] / d1).exp());
        add(c, scale([1.0, 0.97, 0.9], glow * 0.12 * (1.0 - (-dt[i] / d2).exp())))
    });
    let bloom = blur_img(&glass, ctx.px(10.0));
    let lw = ctx.px(p.get("lead")).max(0.6);
    let out = glass.map_xy(|x, y, c| {
        let i = y * w + x;
        let b = bloom.px[i];
        let c = add(c, scale([(b[0] - 0.45).max(0.0), (b[1] - 0.45).max(0.0), (b[2] - 0.45).max(0.0)], glow * 0.6));
        let a = smoothstep(lw + 0.8, lw - 0.6, dt[i]);
        let profile = (1.0 - dt[i] / lw).max(0.0).sqrt();
        let lc = 0.03 + 0.13 * profile;
        clamp01(mix(c, [lc, lc * 0.98, lc * 0.95], a))
    });
    Some(out)
}

// ------------------------------------------------------------------ low poly

pub fn lowpoly(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let n = p.get("points") as usize;
    let focus = p.get("focus");
    let light = p.get("light");
    let outline = p.get("outline");
    let mut rng = Rng::new(ctx.seed);
    let base = blur_img(src, ctx.px(1.5));
    let lum = base.luma();
    let grad = blur(&gradient_mag(&lum, w, h), w, h, ctx.px(1.5));
    let mean = grad.iter().sum::<f32>() / grad.len() as f32;
    let gnorm = (mean * 4.0).max(1e-4);

    let (wf, hf) = ((w - 1) as f64, (h - 1) as f64);
    let mut pts: Vec<delaunator::Point> = Vec::with_capacity(n + 64);
    let edge_n = 12;
    for k in 0..=edge_n {
        let t = k as f64 / edge_n as f64;
        pts.push(delaunator::Point { x: t * wf, y: 0.0 });
        pts.push(delaunator::Point { x: t * wf, y: hf });
        if k > 0 && k < edge_n {
            pts.push(delaunator::Point { x: 0.0, y: t * hf });
            pts.push(delaunator::Point { x: wf, y: t * hf });
        }
    }
    let mut tries = 0;
    while pts.len() < n && tries < n * 60 {
        tries += 1;
        let (x, y) = (rng.range(0.0, wf as f32), rng.range(0.0, hf as f32));
        let g = (grad[y as usize * w + x as usize] / gnorm).min(1.0);
        if rng.f32() < (1.0 - focus) + focus * g {
            pts.push(delaunator::Point { x: x as f64, y: y as f64 });
        }
    }
    ctx.progress(0.3);
    let tri = delaunator::triangulate(&pts);
    let lp = |i: usize| [pts[i].x as f32, pts[i].y as f32];
    let ldir = [-0.6f32, -0.8];

    let faces: Vec<([[f32; 2]; 3], Rgb)> = tri
        .triangles
        .par_chunks(3)
        .map(|t| {
            let v = [lp(t[0]), lp(t[1]), lp(t[2])];
            let cen = [(v[0][0] + v[1][0] + v[2][0]) / 3.0, (v[0][1] + v[1][1] + v[2][1]) / 3.0];
            let mut c = base.sample(cen[0], cen[1]);
            for vv in &v {
                c = add(c, base.sample(vv[0] * 0.33 + cen[0] * 0.67, vv[1] * 0.33 + cen[1] * 0.67));
            }
            let c = scale(c, 0.25);
            // Fit a plane to luminance at the corners -> facet orientation.
            let l: Vec<f32> = v.iter().map(|vv| luma(base.sample(vv[0], vv[1]))).collect();
            let (e1, e2) = ([v[1][0] - v[0][0], v[1][1] - v[0][1]], [v[2][0] - v[0][0], v[2][1] - v[0][1]]);
            let det = e1[0] * e2[1] - e1[1] * e2[0];
            let (gx, gy) = if det.abs() > 1e-3 {
                let (d1, d2) = (l[1] - l[0], l[2] - l[0]);
                ((d1 * e2[1] - d2 * e1[1]) / det, (e1[0] * d2 - e2[0] * d1) / det)
            } else {
                (0.0, 0.0)
            };
            let k = 60.0 * ctx.scale;
            let shade = 1.0
                + light * ((gx * ldir[0] + gy * ldir[1]) * k).clamp(-0.35, 0.35)
                + light * 0.1 * (hash2(t[0] as i32, t[1] as i32, 3) - 0.5);
            (v, clamp01(scale(c, shade)))
        })
        .collect();
    ctx.progress(0.6);

    let mut out = Img::new(w, h, [0.0, 0.0, 0.0]);
    let ow = ctx.px(0.9);
    for (v, c) in &faces {
        let minx = v.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let maxx = (v.iter().map(|p| p[0]).fold(0.0, f32::max).ceil() as usize).min(w - 1);
        let miny = v.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let maxy = (v.iter().map(|p| p[1]).fold(0.0, f32::max).ceil() as usize).min(h - 1);
        let edge = |a: [f32; 2], b: [f32; 2], px: f32, py: f32| (b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0]);
        let area = edge(v[0], v[1], v[2][0], v[2][1]);
        if area.abs() < 1e-6 {
            continue;
        }
        let lens = [
            (v[1][0] - v[0][0]).hypot(v[1][1] - v[0][1]),
            (v[2][0] - v[1][0]).hypot(v[2][1] - v[1][1]),
            (v[0][0] - v[2][0]).hypot(v[0][1] - v[2][1]),
        ];
        for y in miny..=maxy {
            for x in minx..=maxx {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let e = [edge(v[0], v[1], px, py) / area, edge(v[1], v[2], px, py) / area, edge(v[2], v[0], px, py) / area];
                if e[0] < -1e-4 || e[1] < -1e-4 || e[2] < -1e-4 {
                    continue;
                }
                let mut col = *c;
                if outline > 0.0 {
                    let d = (e[0] * area.abs() / lens[0]).min(e[1] * area.abs() / lens[1]).min(e[2] * area.abs() / lens[2]);
                    let a = smoothstep(ow + 0.7, ow - 0.3, d) * outline;
                    col = mix(col, scale(col, 0.35), a);
                }
                out.px[y * w + x] = col;
            }
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ pop art

const POP: [[u32; 5]; 4] = [
    [0x1d1a4f, 0xe6007e, 0xff8a00, 0xffe600, 0xfff6d5],
    [0x111111, 0x00a19a, 0x9bdc28, 0xff6fb5, 0xf7f3e8],
    [0x3b0f70, 0xe8112d, 0x22c3e6, 0xfff200, 0xfdfdf2],
    [0x2b1a12, 0x1d4ed8, 0xff4d8d, 0x7cf2c4, 0xfff1c1],
];

pub fn popart(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let gap = ctx.px(8.0).round() as usize;
    let (qw, qh) = (((w - gap * 3) / 2).max(8), ((h - gap * 3) / 2).max(8));
    let small = src.resize_exact(qw, qh, image::imageops::FilterType::Triangle);
    let sctx_px = ctx.px(1.0) * qw as f32 / w as f32 * 2.0;
    let l = blur(&small.luma(), qw, qh, (sctx_px * 1.2).max(0.5));
    let mut sorted = l.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (lo, hi) = (sorted[sorted.len() / 50], sorted[sorted.len() * 49 / 50].max(sorted[sorted.len() / 50] + 1e-3));
    let e = xdog(&small.luma(), qw, qh, (sctx_px * 1.1).max(0.6), 22.0, 0.85, 25.0);
    let levels = p.get("levels").round().max(2.0) as usize;
    let (outline, dots) = (p.get("outline"), p.get("dots"));
    let dot_cell = (ctx.px(7.0) * qw as f32 / w as f32 * 2.0).max(3.0);
    ctx.progress(0.4);

    let mut out = Img::new(w, h, hex(0xfaf6ea));
    let offsets = [(gap, gap), (gap * 2 + qw, gap), (gap, gap * 2 + qh), (gap * 2 + qw, gap * 2 + qh)];
    for (panel, &(ox, oy)) in offsets.iter().enumerate() {
        let pal = POP[panel];
        let pick = |idx: usize| hex(pal[(idx * 4 + (levels - 1) / 2) / (levels - 1)]);
        for y in 0..qh {
            for x in 0..qw {
                let i = y * qw + x;
                let t = ((l[i] - lo) / (hi - lo)).clamp(0.0, 0.9999);
                let idx = (t * levels as f32) as usize;
                let mut c = pick(idx);
                if idx > 0 && idx + 1 < levels && dots > 0.0 {
                    let (u, v) = (x as f32 / dot_cell, y as f32 / dot_cell);
                    let d = ((u.fract() - 0.5).powi(2) + (v.fract() - 0.5).powi(2)).sqrt();
                    let inside = smoothstep(0.32, 0.26, d);
                    c = mix(c, pick(idx + 1), (1.0 - inside) * dots * 0.7);
                }
                c = scale(c, 1.0 - (1.0 - e[i]) * outline * 0.9);
                let (tx, ty) = (ox + x, oy + y);
                if tx < w && ty < h {
                    out.px[ty * w + tx] = c;
                }
            }
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ halftone

pub fn halftone(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let cell = ctx.px(p.get("cell")).max(2.0);
    let base = blur_img(src, cell * 0.35);
    let density = p.get("density");
    let off = p.get("angle");
    let mis = ctx.px(p.get("misregister"));
    let inks = [hex(0x00aeef), hex(0xec008c), hex(0xfff200), hex(0x1a1a1a)];
    let angles: Vec<(f32, f32)> = [15.0f32, 75.0, 0.0, 45.0].iter().map(|a| (a + off) * PI / 180.0).map(|a| (a.cos(), a.sin())).collect();
    let shifts = [(mis, 0.0), (-mis * 0.6, mis * 0.8), (0.0, -mis * 0.7), (0.0, 0.0)];
    let paper = [0.97, 0.955, 0.92];
    let aa = 0.8 / cell;
    let out = src.map_xy(|x, y, _| {
        let mut c = paper;
        for k in 0..4 {
            let (ca, sa) = angles[k];
            let (px, py) = (x as f32 - shifts[k].0, y as f32 - shifts[k].1);
            let u = (px * ca + py * sa) / cell;
            let v = (-px * sa + py * ca) / cell;
            let (cu, cv) = (u.floor() + 0.5, v.floor() + 0.5);
            let sx = (cu * ca - cv * sa) * cell;
            let sy = (cu * sa + cv * ca) * cell;
            let s = base.sample(sx, sy);
            let kk = (1.0 - s[0].max(s[1]).max(s[2])) * 0.7;
            let cov = if k == 3 {
                kk
            } else {
                let ch = s[k];
                ((1.0 - ch - kk) / (1.0 - kk).max(1e-3)).clamp(0.0, 1.0)
            };
            let cov = (cov * density).clamp(0.0, 1.0);
            let r = (cov / PI).sqrt() * 1.12;
            let d = ((u - cu).powi(2) + (v - cv).powi(2)).sqrt();
            let a = smoothstep(r + aa, r - aa, d);
            c = mul(c, mix([1.0, 1.0, 1.0], inks[k], a * 0.92));
        }
        c
    });
    Some(out)
}

// ------------------------------------------------------------------ risograph

const RISO: [(u32, u32); 5] =
    [(0xff48b0, 0x0078bf), (0x00838a, 0xff6c2f), (0xf15060, 0x3d5588), (0x00a95c, 0x765ba7), (0xffe800, 0x435060)];

pub fn riso(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
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

// ------------------------------------------------------------------ pixel art

fn kmeans(data: &[Rgb], k: usize, rng: &mut Rng) -> Vec<Rgb> {
    let k = k.min(data.len()).max(1);
    let mut cent = vec![data[rng.below(data.len())]];
    let mut d: Vec<f32> = data.iter().map(|&c| dist2(c, cent[0])).collect();
    while cent.len() < k {
        let total: f32 = d.iter().sum();
        let mut t = rng.f32() * total;
        let mut pick = data.len() - 1;
        for (i, &v) in d.iter().enumerate() {
            t -= v;
            if t <= 0.0 {
                pick = i;
                break;
            }
        }
        cent.push(data[pick]);
        for (i, &c) in data.iter().enumerate() {
            d[i] = d[i].min(dist2(c, data[pick]));
        }
    }
    for _ in 0..12 {
        let assign: Vec<usize> = data.par_iter().map(|&c| nearest(&cent, c)).collect();
        let mut sums = vec![[0.0f32; 4]; k];
        for (c, &a) in data.iter().zip(assign.iter()) {
            sums[a][0] += c[0];
            sums[a][1] += c[1];
            sums[a][2] += c[2];
            sums[a][3] += 1.0;
        }
        for (j, s) in sums.iter().enumerate() {
            if s[3] > 0.0 {
                cent[j] = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
    }
    cent
}

fn nearest(pal: &[Rgb], c: Rgb) -> usize {
    let mut best = 0;
    let mut bd = f32::INFINITY;
    for (i, &p) in pal.iter().enumerate() {
        let d = dist2(p, c);
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

pub fn pixel(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let ps = ctx.px(p.get("size")).round().max(2.0) as usize;
    let (sw, sh) = (w.div_ceil(ps), h.div_ceil(ps));
    let sat = p.get("saturation");
    let small: Vec<Rgb> = (0..sw * sh)
        .into_par_iter()
        .map(|i| {
            let (sx, sy) = (i % sw, i / sw);
            let mut acc = [0.0f32; 3];
            let mut n = 0.0;
            for y in sy * ps..((sy + 1) * ps).min(h) {
                for x in sx * ps..((sx + 1) * ps).min(w) {
                    acc = add(acc, src.at(x, y));
                    n += 1.0;
                }
            }
            saturate(scale(acc, 1.0 / n), sat)
        })
        .collect();
    let mut rng = Rng::new(ctx.seed);
    let pal = kmeans(&small, p.get("colors") as usize, &mut rng);
    ctx.progress(0.6);
    const BAYER: [f32; 16] = [0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.];
    let dither = p.get("dither");
    let q: Vec<Rgb> = small
        .par_iter()
        .enumerate()
        .map(|(i, &c)| {
            let (x, y) = (i % sw, i / sw);
            let b = (BAYER[(y % 4) * 4 + x % 4] + 0.5) / 16.0 - 0.5;
            let o = b * dither * 0.22;
            pal[nearest(&pal, [c[0] + o, c[1] + o, c[2] + o])]
        })
        .collect();
    let outline = p.get("outline");
    let q2: Vec<Rgb> = (0..sw * sh)
        .map(|i| {
            let (x, y) = (i % sw, i / sw);
            let l = luma(q[i]);
            let mut edge = false;
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx >= 0 && ny >= 0 && (nx as usize) < sw && (ny as usize) < sh {
                    let nl = luma(q[ny as usize * sw + nx as usize]);
                    if nl - l > 0.22 {
                        edge = true;
                    }
                }
            }
            if edge { scale(q[i], 1.0 - 0.5 * outline) } else { q[i] }
        })
        .collect();
    Some(src.map_xy(|x, y, _| q2[(y / ps) * sw + x / ps]))
}
