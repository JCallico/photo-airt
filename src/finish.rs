//! Non-destructive finishing layer applied on top of any artwork
//! (algorithmic or AI generated): grade, vignette, grain, canvas weave, glow.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::imaging::*;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Finish {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub warmth: f32,
    pub vignette: f32,
    pub grain: f32,
    pub canvas: f32,
    pub glow: f32,
}

impl Default for Finish {
    fn default() -> Self {
        Self { exposure: 0.0, contrast: 0.0, saturation: 0.0, warmth: 0.0, vignette: 0.0, grain: 0.0, canvas: 0.0, glow: 0.0 }
    }
}

pub struct FinishSpec {
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
}

impl Finish {
    pub fn is_identity(&self) -> bool {
        *self == Finish::default()
    }

    pub fn fields_mut(&mut self) -> [(&'static FinishSpec, &mut f32); 8] {
        const S: [FinishSpec; 8] = [
            FinishSpec { label: "Exposure", min: -1.0, max: 1.0 },
            FinishSpec { label: "Contrast", min: -1.0, max: 1.0 },
            FinishSpec { label: "Saturation", min: -1.0, max: 1.0 },
            FinishSpec { label: "Warmth", min: -1.0, max: 1.0 },
            FinishSpec { label: "Vignette", min: 0.0, max: 1.0 },
            FinishSpec { label: "Film grain", min: 0.0, max: 1.0 },
            FinishSpec { label: "Canvas weave", min: 0.0, max: 1.0 },
            FinishSpec { label: "Glow", min: 0.0, max: 1.0 },
        ];
        [
            (&S[0], &mut self.exposure),
            (&S[1], &mut self.contrast),
            (&S[2], &mut self.saturation),
            (&S[3], &mut self.warmth),
            (&S[4], &mut self.vignette),
            (&S[5], &mut self.grain),
            (&S[6], &mut self.canvas),
            (&S[7], &mut self.glow),
        ]
    }

    pub fn sanitized(mut self) -> Self {
        for (spec, v) in self.fields_mut() {
            *v = if v.is_finite() { v.clamp(spec.min, spec.max) } else { 0.0 };
        }
        self
    }

    pub fn apply(&self, img: &Img) -> Img {
        if self.is_identity() {
            return img.clone();
        }
        let (w, h) = (img.w, img.h);
        let scale_px = img.long_side() as f32 / 1600.0;
        let bloom = if self.glow > 0.0 { Some(blur_img(img, (14.0 * scale_px).max(1.0))) } else { None };
        let weave = (3.2 * scale_px).max(1.5);
        let exp = 2f32.powf(self.exposure * 1.2);
        let con = 1.0 + self.contrast * 0.8;
        let sat = 1.0 + self.saturation;
        let warm = [1.0 + self.warmth * 0.12, 1.0 + self.warmth * 0.02, 1.0 - self.warmth * 0.12];
        let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
        let rmax = (cx * cx + cy * cy).sqrt();
        let px = img
            .px
            .par_iter()
            .enumerate()
            .map(|(i, &c)| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let mut c = mul(scale(c, exp), warm);
                c = [(c[0] - 0.5) * con + 0.5, (c[1] - 0.5) * con + 0.5, (c[2] - 0.5) * con + 0.5];
                c = saturate(clamp01(c), sat);
                if let Some(b) = &bloom {
                    let b = b.px[i];
                    let lift = |v: f32| (v - 0.55).max(0.0) * 1.6;
                    let g = [lift(b[0]), lift(b[1]), lift(b[2])];
                    c = [
                        1.0 - (1.0 - c[0]) * (1.0 - g[0] * self.glow),
                        1.0 - (1.0 - c[1]) * (1.0 - g[1] * self.glow),
                        1.0 - (1.0 - c[2]) * (1.0 - g[2] * self.glow),
                    ];
                }
                if self.canvas > 0.0 {
                    let u = (x / weave * std::f32::consts::PI).sin();
                    let v = (y / weave * std::f32::consts::PI).sin();
                    let thread = if (((x / weave) as i32 + (y / weave) as i32) & 1) == 0 { u * u } else { v * v };
                    let n = value_noise(x / 1.7, y / 1.7, 99) - 0.5;
                    c = scale(c, 1.0 + self.canvas * ((thread - 0.5) * 0.14 + n * 0.06));
                }
                if self.vignette > 0.0 {
                    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / rmax;
                    c = scale(c, 1.0 - self.vignette * smoothstep(0.35, 1.05, d) * 0.75);
                }
                if self.grain > 0.0 {
                    let n = hash2(x as i32, y as i32, 1234) - 0.5;
                    let l = luma(c);
                    let k = self.grain * 0.22 * (1.0 - (l - 0.5).abs() * 1.2);
                    c = add(c, [n * k, n * k, n * k]);
                }
                clamp01(c)
            })
            .collect();
        Img { w, h, px }
    }
}
