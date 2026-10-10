//! CMYK Halftone: one of the styles built into Photo·AIrt.
//!
//! Rosette dot screens of a vintage four-colour press.

use std::f32::consts::PI;

use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};

/// This style: its description and its render function.
pub const STYLE: Style = Style { description, render };

/// This style's description, from its own `plugin.toml`.
pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
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
