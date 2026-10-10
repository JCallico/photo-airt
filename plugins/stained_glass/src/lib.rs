//! Stained Glass: one of the styles built into Photo·AIrt.
//!
//! Leaded glass panes glowing with the colours of your scene.

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
