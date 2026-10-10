//! Oil on Canvas: one of the styles built into Photo·AIrt.
//!
//! Thick, sculpted paint that follows the forms of your photo.

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
