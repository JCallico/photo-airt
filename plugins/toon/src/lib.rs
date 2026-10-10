//! Cel Animation: one of the styles built into Photo·AIrt.
//!
//! Flat shading bands and bold outlines, like a hand-inked frame.

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
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
