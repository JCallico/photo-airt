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

#[cfg(test)]
mod tests {
    use super::*;

    /// A left-to-right grey ramp with a warm tint.
    fn ramp(w: usize, h: usize) -> Img {
        let mut img = Img::new(w, h, [0.0; 3]);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1) as f32;
                img.px[y * w + x] = [v, v * 0.8, v * 0.6];
            }
        }
        img
    }

    fn mean(img: &Img, channel: usize) -> f32 {
        img.px.iter().map(|c| c[channel]).sum::<f32>() / img.px.len() as f32
    }

    #[test]
    fn the_default_finish_leaves_the_artwork_untouched() {
        let img = ramp(32, 16);
        assert!(Finish::default().is_identity());
        assert!(Finish::default().apply(&img).px == img.px);
        assert!(!Finish { glow: 0.1, ..Default::default() }.is_identity());
    }

    #[test]
    fn sanitizing_clamps_each_field_to_its_range_and_drops_nonsense() {
        let f = Finish {
            exposure: 9.0,
            contrast: -9.0,
            saturation: f32::NAN,
            warmth: f32::INFINITY,
            vignette: -1.0,
            grain: 2.0,
            canvas: 0.5,
            glow: f32::NEG_INFINITY,
        }
        .sanitized();
        assert_eq!((f.exposure, f.contrast, f.saturation, f.warmth), (1.0, -1.0, 0.0, 0.0));
        assert_eq!((f.vignette, f.grain, f.canvas, f.glow), (0.0, 1.0, 0.5, 0.0));
    }

    #[test]
    fn each_field_has_a_label_and_a_range_holding_its_default() {
        let mut f = Finish::default();
        let fields = f.fields_mut();
        assert_eq!(fields.len(), 8);
        for (spec, value) in fields {
            assert!(!spec.label.is_empty());
            assert!((spec.min..=spec.max).contains(value), "{}", spec.label);
        }
    }

    #[test]
    fn recipes_may_name_only_some_fields() {
        let f: Finish = serde_json::from_str(r#"{"grain": 0.4}"#).unwrap();
        assert_eq!(f, Finish { grain: 0.4, ..Default::default() });
    }

    #[test]
    fn exposure_contrast_saturation_and_warmth_do_what_they_say() {
        let img = ramp(64, 8);
        let base = mean(&img, 0);
        assert!(mean(&Finish { exposure: 0.5, ..Default::default() }.apply(&img), 0) > base);
        assert!(mean(&Finish { exposure: -0.5, ..Default::default() }.apply(&img), 0) < base);

        let contrast = Finish { contrast: 1.0, ..Default::default() }.apply(&img);
        assert!(contrast.at(5, 0)[0] < img.at(5, 0)[0] && contrast.at(58, 0)[0] > img.at(58, 0)[0]);

        let grey = Finish { saturation: -1.0, ..Default::default() }.apply(&img);
        assert!(grey.px.iter().all(|c| (c[0] - c[1]).abs() < 1e-4 && (c[1] - c[2]).abs() < 1e-4));

        let warm = Finish { warmth: 1.0, ..Default::default() }.apply(&Img::new(8, 8, [0.5; 3]));
        assert!(warm.at(0, 0)[0] > 0.5 && warm.at(0, 0)[2] < 0.5);
    }

    #[test]
    fn the_vignette_darkens_corners_and_spares_the_centre() {
        let img = Img::new(64, 64, [0.8; 3]);
        let out = Finish { vignette: 1.0, ..Default::default() }.apply(&img);
        assert!(out.at(0, 0)[0] < 0.4, "corner {}", out.at(0, 0)[0]);
        assert!((out.at(32, 32)[0] - 0.8).abs() < 1e-4, "centre {}", out.at(32, 32)[0]);
    }

    #[test]
    fn grain_and_canvas_add_repeatable_texture() {
        let img = Img::new(48, 48, [0.5; 3]);
        for f in [Finish { grain: 1.0, ..Default::default() }, Finish { canvas: 1.0, ..Default::default() }] {
            let (a, b) = (f.apply(&img), f.apply(&img));
            assert!(a.px == b.px, "the same finish must give the same picture");
            assert!(a.px.iter().any(|c| (c[0] - 0.5).abs() > 0.01), "no visible texture");
            assert!(a.px.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
        }
    }

    #[test]
    fn glow_blooms_only_from_highlights() {
        let mut bright = Img::new(64, 64, [0.5; 3]);
        for y in 24..40 {
            for x in 24..40 {
                bright.px[y * 64 + x] = [1.0; 3];
            }
        }
        let glow = Finish { glow: 1.0, ..Default::default() };
        let out = glow.apply(&bright);
        // One pixel outside the bright square, the light spills over.
        assert!(out.at(23, 32)[0] > 0.505, "{}", out.at(23, 32)[0]);
        assert!((out.at(4, 4)[0] - 0.5).abs() < 1e-3, "too far to reach");
        let dim = Img::new(64, 64, [0.5; 3]);
        assert!(glow.apply(&dim).px.iter().all(|c| (c[0] - 0.5).abs() < 1e-3), "nothing bright enough to bloom");
    }

    #[test]
    fn a_finish_keeps_the_image_size() {
        let f = Finish { exposure: 0.2, contrast: 0.2, saturation: 0.2, warmth: 0.2, vignette: 0.2, grain: 0.2, canvas: 0.2, glow: 0.2 };
        let out = f.apply(&ramp(40, 20));
        assert_eq!((out.w, out.h, out.px.len()), (40, 20, 800));
    }
}
