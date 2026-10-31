//! Stained Glass: one of the styles built into Photo·AIrt.
//!
//! Leaded glass panes glowing with the colours of your scene.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};
use rayon::prelude::*;

/// How strongly panes keep compact shapes rather than hugging colour edges.
/// A few SLIC iterations keep the seeds' irregular, hand-cut look; many would
/// relax the panes into a regular grid.
const COMPACTNESS: f32 = 0.3;

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

/// One glass pane while the panes are being fitted: centre, mean colour and
/// the pane size it was seeded at.
#[derive(Clone, Copy)]
struct Pane {
    x: f32,
    y: f32,
    c: Rgb,
    s: f32,
}

/// Fit the panes to the photo with SLIC superpixels (Achanta et al. 2012):
/// each pixel joins the nearby pane that best balances colour likeness
/// against distance, so the lead follows the edges of the scene. `compact`
/// weighs distance against colour; panes may differ in size.
fn slic(img: &Img, panes: &mut [Pane], bin: f32, compact: f32, iters: usize, ctx: &Ctx) -> Option<Vec<u32>> {
    let (w, h) = (img.w, img.h);
    let (bw, bh) = ((w as f32 / bin).ceil() as usize, (h as f32 / bin).ceil() as usize);
    let bin_of = |x: f32, y: f32| ((x / bin) as usize).min(bw - 1) + ((y / bin) as usize).min(bh - 1) * bw;
    let mut labels = vec![0u32; w * h];
    for it in 0..iters {
        if ctx.cancelled() {
            return None;
        }
        let mut bins = vec![Vec::new(); bw * bh];
        for (k, pn) in panes.iter().enumerate() {
            bins[bin_of(pn.x, pn.y)].push(k as u32);
        }
        labels.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let by = ((y as f32 / bin) as usize).min(bh - 1);
            for (x, o) in row.iter_mut().enumerate() {
                let bx = ((x as f32 / bin) as usize).min(bw - 1);
                let c = img.at(x, y);
                let (mut best, mut bd) = (*o, f32::INFINITY);
                for ny in by.saturating_sub(1)..=(by + 1).min(bh - 1) {
                    for nx in bx.saturating_sub(1)..=(bx + 1).min(bw - 1) {
                        for &k in &bins[ny * bw + nx] {
                            let pn = &panes[k as usize];
                            let (dx, dy) = (x as f32 - pn.x, y as f32 - pn.y);
                            let d = dist2(c, pn.c) + compact * compact * (dx * dx + dy * dy) / (pn.s * pn.s);
                            if d < bd {
                                bd = d;
                                best = k;
                            }
                        }
                    }
                }
                *o = best;
            }
        });
        let mut sums = vec![[0.0f32; 6]; panes.len()];
        for (i, &k) in labels.iter().enumerate() {
            let (c, sm) = (img.px[i], &mut sums[k as usize]);
            *sm = [sm[0] + (i % w) as f32, sm[1] + (i / w) as f32, sm[2] + c[0], sm[3] + c[1], sm[4] + c[2], sm[5] + 1.0];
        }
        for (pn, sm) in panes.iter_mut().zip(&sums) {
            if sm[5] > 0.0 {
                let n = 1.0 / sm[5];
                (pn.x, pn.y, pn.c) = (sm[0] * n, sm[1] * n, [sm[2] * n, sm[3] * n, sm[4] * n]);
            }
        }
        ctx.progress(0.1 + 0.45 * (it + 1) as f32 / iters as f32);
    }
    Some(labels)
}

/// Relabel connected regions so every pane is one piece; fragments smaller
/// than `min_size` pixels join the pane they touch. Returns the new labels
/// and their count.
fn connect(labels: &[u32], w: usize, h: usize, min_size: usize) -> (Vec<u32>, usize) {
    let mut out = vec![u32::MAX; w * h];
    let (mut next, mut stack, mut comp) = (0u32, Vec::new(), Vec::new());
    let neighbours = |i: usize| {
        let (x, y) = (i % w, i / w);
        [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)]
    };
    for start in 0..w * h {
        if out[start] != u32::MAX {
            continue;
        }
        let adjacent = neighbours(start).into_iter().flatten().map(|n| out[n]).find(|&l| l != u32::MAX);
        let lab = labels[start];
        out[start] = next;
        comp.clear();
        stack.push(start);
        while let Some(i) = stack.pop() {
            comp.push(i);
            for n in neighbours(i).into_iter().flatten() {
                if out[n] == u32::MAX && labels[n] == lab {
                    out[n] = next;
                    stack.push(n);
                }
            }
        }
        match adjacent {
            Some(a) if comp.len() < min_size => comp.iter().for_each(|&i| out[i] = a),
            _ => next += 1,
        }
    }
    (out, next as usize)
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let seed = ctx.seed32();
    let cell = ctx.px(p.get("cell")).max(4.0);
    let adapt = p.get("adaptivity");
    let mut rng = Rng::new(ctx.seed);

    // Seed one pane per cell, or four smaller ones where the photo has detail.
    let grad = gradient_mag(&src.luma(), w, h);
    let grad = blur(&grad, w, h, cell * 0.5);
    let smooth = blur_img(src, ctx.px(1.0));
    let mut panes: Vec<Pane> = Vec::new();
    let mut pane = |x: f32, y: f32, s: f32| {
        let (x, y) = (x.min(w as f32 - 1.0), y.min(h as f32 - 1.0));
        panes.push(Pane { x, y, c: smooth.at(x as usize, y as usize), s });
    };
    let mut y = 0.0;
    while y < h as f32 {
        let mut x = 0.0;
        while x < w as f32 {
            let (cx, cy) = ((x + cell * 0.5).min(w as f32 - 1.0), (y + cell * 0.5).min(h as f32 - 1.0));
            let detail = (grad[cy as usize * w + cx as usize] * 9.0).min(1.0);
            if rng.f32() < adapt * detail {
                for (qx, qy) in [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)] {
                    pane(x + (qx + rng.range(0.1, 0.4)) * cell, y + (qy + rng.range(0.1, 0.4)) * cell, cell * 0.5);
                }
            } else {
                pane(x + rng.range(0.2, 0.8) * cell, y + rng.range(0.2, 0.8) * cell, cell);
            }
            x += cell;
        }
        y += cell;
    }
    ctx.progress(0.1);

    let labels = slic(&smooth, &mut panes, cell, COMPACTNESS, 3, ctx)?;
    let (ids, count) = connect(&labels, w, h, (cell * cell / 16.0) as usize);
    ctx.progress(0.6);

    let mut sums = vec![[0.0f32; 4]; count];
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
        c = scale(c, 1.08 - 0.12 * (-dt[i] / d1).exp());
        add(c, scale([1.0, 0.97, 0.9], glow * 0.12 * (1.0 - (-dt[i] / d2).exp())))
    });
    ctx.snapshot(&glass);
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
