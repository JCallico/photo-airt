//! Pop Art Quad: one of the styles built into Photo·AIrt.
//!
//! Four screen-printed panels in clashing pop palettes.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

const POP: [[u32; 5]; 4] = [
    [0x1d1a4f, 0xe6007e, 0xff8a00, 0xffe600, 0xfff6d5],
    [0x111111, 0x00a19a, 0x9bdc28, 0xff6fb5, 0xf7f3e8],
    [0x3b0f70, 0xe8112d, 0x22c3e6, 0xfff200, 0xfdfdf2],
    [0x2b1a12, 0x1d4ed8, 0xff4d8d, 0x7cf2c4, 0xfff1c1],
];

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let (w, h) = (src.w, src.h);
    let gap = ctx.px(8.0).round() as usize;
    let (qw, qh) = (((w - gap * 3) / 2).max(8), ((h - gap * 3) / 2).max(8));
    let small = src.resize_exact(qw, qh, FilterType::Triangle);
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
