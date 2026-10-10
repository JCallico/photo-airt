//! Low Poly: one of the styles built into Photo·AIrt.
//!
//! Faceted triangles that crystallise the scene.

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
