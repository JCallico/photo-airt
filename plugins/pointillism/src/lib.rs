//! Pointillism: one of the styles built into Photo·AIrt.
//!
//! Seurat-style dabs of pure colour that mix in the eye.

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
    let dot = ctx.px(p.get("dot")).max(1.0);
    let spacing = (dot * 0.75 / p.get("density")).max(0.8);
    let jitter = p.get("jitter");
    let div = p.get("divisionism");
    let sat = p.get("saturation");
    let base = blur_img(src, spacing * 0.5);
    // Toned ground: gaps between dabs read as underpainting, not bare paper.
    let mut canvas = blur_img(&base, spacing * 2.0).map(|c| mix(c, [0.96, 0.94, 0.88], 0.3));
    let mut rng = Rng::new(ctx.seed);

    let dab = |rng: &mut Rng, x: f32, y: f32| -> Rgb {
        let c = saturate(base.sample(x, y), sat);
        let mut hsv = rgb_to_hsv(c);
        hsv[0] += rng.range(-jitter, jitter) * 0.3;
        hsv[2] *= 1.0 + rng.range(-jitter, jitter);
        if rng.f32() < div {
            // Divisionism: split the colour into neighbouring pure hues.
            hsv[0] += if rng.f32() < 0.5 { 0.07 } else { -0.07 };
            hsv[1] = (hsv[1] * 1.4).min(1.0);
        }
        clamp01(hsv_to_rgb(hsv))
    };

    let mut pts = Vec::new();
    let mut y = 0.0;
    while y < h as f32 {
        let mut x = 0.0;
        while x < w as f32 {
            pts.push([x + rng.range(0.0, spacing), y + rng.range(0.0, spacing)]);
            x += spacing;
        }
        y += spacing;
    }
    rng.shuffle(&mut pts);
    let total = pts.len();
    for (k, pt) in pts.into_iter().enumerate() {
        let c = dab(&mut rng, pt[0], pt[1]);
        let r = (dot * 0.5).max(spacing * 0.72) * rng.range(0.85, 1.2);
        stamp_disc(&mut canvas, pt[0], pt[1], r, c, 0.93);
        if k % 20000 == 0 {
            if ctx.cancelled() {
                return None;
            }
            ctx.progress(0.1 + 0.6 * k as f32 / total as f32);
        }
    }

    // Detail pass: smaller dabs where the photo has structure.
    let grad = gradient_mag(&src.luma(), w, h);
    let grad = blur(&grad, w, h, spacing * 0.5);
    let fine = spacing * 0.55;
    let mut pts = Vec::new();
    let mut y = 0.0;
    while y < h as f32 {
        let mut x = 0.0;
        while x < w as f32 {
            let (px, py) = (x + rng.range(0.0, fine), y + rng.range(0.0, fine));
            let g = grad[(py as usize).min(h - 1) * w + (px as usize).min(w - 1)];
            if g > 0.04 && rng.f32() < (g * 10.0).min(1.0) {
                pts.push([px, py]);
            }
            x += fine;
        }
        y += fine;
    }
    rng.shuffle(&mut pts);
    for pt in pts {
        let c = dab(&mut rng, pt[0], pt[1]);
        stamp_disc(&mut canvas, pt[0], pt[1], dot * 0.3 * rng.range(0.8, 1.2), c, 0.95);
    }
    ctx.progress(0.98);
    Some(canvas)
}
