use rayon::prelude::*;

use super::painterly::akf;
use super::{Ctx, Params};
use crate::imaging::*;

/// eXtended Difference of Gaussians, line mode: ~1 on flat areas, dark lines
/// on the dark side of edges. `p` = edge emphasis, `eps` = threshold,
/// `phi` = steepness of the tanh ramp.
pub(crate) fn xdog(l: &[f32], w: usize, h: usize, sigma: f32, p: f32, eps: f32, phi: f32) -> Vec<f32> {
    xdog_fill(l, w, h, sigma, p, eps, phi, 0.0)
}

/// `fill` blends between pure lines (0) and classic XDoG tonal fills (1).
#[allow(clippy::too_many_arguments)]
pub(crate) fn xdog_fill(l: &[f32], w: usize, h: usize, sigma: f32, p: f32, eps: f32, phi: f32, fill: f32) -> Vec<f32> {
    let g1 = blur(l, w, h, sigma);
    let g2 = blur(l, w, h, sigma * 1.6);
    g1.par_iter()
        .zip(g2.par_iter())
        .map(|(&a, &b)| {
            let s = (1.0 + (a - 1.0) * fill) + p * (a - b);
            if s >= eps { 1.0 } else { (1.0 + (phi * (s - eps)).tanh()).max(0.0) }
        })
        .collect()
}

pub fn pencil(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let seed = ctx.seed32();
    let l = src.luma();
    let inv: Vec<f32> = l.iter().map(|v| 1.0 - v).collect();
    let b = blur(&inv, w, h, ctx.px(p.get("weight")));
    ctx.progress(0.2);
    let darkness = p.get("darkness");
    let dodge: Vec<f32> =
        l.par_iter().zip(b.par_iter()).map(|(&v, &bb)| (v / (1.0 - bb).max(0.02)).min(1.0).powf(1.0 + 2.5 * darkness)).collect();

    let flow = flow_field(src, ctx.px(4.0));
    let cell = ctx.px(1.0).max(1.0);
    let noise: Vec<f32> =
        (0..w * h).into_par_iter().map(|i| hash2(((i % w) as f32 / cell) as i32, ((i / w) as f32 / cell) as i32, seed)).collect();
    if ctx.cancelled() {
        return None;
    }
    let hatch = lic(&noise, &flow, w, h, ctx.px(12.0));
    ctx.progress(0.7);
    let l_soft = blur(&l, w, h, ctx.px(3.0));
    let (hatching, color, paper) = (p.get("hatching"), p.get("color"), p.get("paper"));
    let out = src.map_xy(|x, y, c| {
        let i = y * w + x;
        let hs = ((hatch[i] - 0.5) * 3.0 + 0.5).clamp(0.0, 1.0);
        let mut v = dodge[i] - hatching * (1.0 - l_soft[i]) * 0.55 * hs;
        let tooth = value_noise(x as f32 / 1.4, y as f32 / 1.4, seed + 5);
        v = 1.0 - (1.0 - v.clamp(0.0, 1.0)) * (1.0 - paper * 0.45 * tooth);
        let li = l[i].max(0.05);
        let chroma = [(c[0] / li).min(2.0), (c[1] / li).min(2.0), (c[2] / li).min(2.0)];
        let tint = mix([1.0, 1.0, 1.0], chroma, color);
        let g = [v * 0.97, v * 0.97, v];
        let shade = mix(g, mul(g, tint), (1.0 - v * 0.5) * color.min(1.0) + color * 0.3);
        mul(clamp01(shade), [0.985, 0.975, 0.95])
    });
    ctx.progress(0.98);
    Some(out)
}

pub fn ink(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let seed = ctx.seed32();
    let l = src.luma();
    let soft = p.get("softness");
    let e = xdog(&l, w, h, ctx.px(p.get("detail")).max(0.5), p.get("strength"), p.get("threshold"), 80.0 - 74.0 * soft);
    ctx.progress(0.5);
    let lb = blur(&l, w, h, ctx.px(3.0));
    let wash = p.get("wash");
    let sepia = p.get("sepia");
    let paper = mix([0.97, 0.97, 0.96], [0.96, 0.91, 0.8], sepia);
    let ink = mix([0.05, 0.05, 0.07], [0.2, 0.11, 0.05], sepia);
    let turb = ctx.px(90.0);
    let out = src.map_xy(|x, y, _| {
        let i = y * w + x;
        let t = lb[i] * 4.0;
        let f = t.floor();
        let q = (f + smoothstep(0.35, 0.65, t - f)) / 4.0;
        let tn = fbm(x as f32 / turb, y as f32 / turb, 4, seed) - 0.5;
        let wash_v = (1.0 - (1.0 - q) * 1.1 * wash * (1.0 + tn * 0.6)).clamp(0.0, 1.0);
        let base = mix(mix(paper, ink, 0.55), paper, wash_v);
        mix(ink, base, e[i])
    });
    Some(out)
}

pub fn toon(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let flow = flow_field(src, ctx.px(2.0));
    let abs = akf(src, &flow, ctx.px(p.get("smoothing")).max(1.2), 8.0, 1.0, ctx, 0.02, 0.75)?;
    let q = p.get("levels").round().max(2.0);
    let sat = p.get("saturation");
    let outline = p.get("outline");
    let l = blur(&src.luma(), w, h, ctx.px(0.6).max(0.5));
    let e = xdog(&l, w, h, ctx.px(1.3).max(0.7), 24.0, 0.86, 30.0);
    let out = abs.map_xy(|x, y, c| {
        let mut hsv = rgb_to_hsv(c);
        let t = hsv[2] * q;
        let f = t.floor();
        hsv[2] = ((f + smoothstep(0.42, 0.58, t - f)) / q).min(1.0);
        let c = saturate(hsv_to_rgb(hsv), sat);
        scale(c, 1.0 - (1.0 - e[y * w + x]) * outline)
    });
    Some(out)
}
