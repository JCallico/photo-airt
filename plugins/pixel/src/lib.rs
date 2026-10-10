//! Pixel Art: one of the styles built into Photo·AIrt.
//!
//! A limited-palette sprite of your photo, 16-bit style.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};
use rayon::prelude::*;

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

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

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
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
