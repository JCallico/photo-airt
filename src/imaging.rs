//! Core raster primitives shared by every style: a linear float RGB buffer,
//! fast blurs, structure-tensor flow fields, line integral convolution,
//! distance transforms, noise and a tiny deterministic RNG.

use rayon::prelude::*;

pub type Rgb = [f32; 3];

#[derive(Clone)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Rgb>,
}

impl Img {
    pub fn new(w: usize, h: usize, fill: Rgb) -> Self {
        Self { w, h, px: vec![fill; w * h] }
    }

    pub fn from_rgb8(img: &image::RgbImage) -> Self {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let px = img.as_raw().par_chunks_exact(3).map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]).collect();
        Self { w, h, px }
    }

    pub fn to_rgb8(&self) -> image::RgbImage {
        let raw: Vec<u8> = self.px.par_iter().flat_map_iter(|c| c.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
        image::RgbImage::from_raw(self.w as u32, self.h as u32, raw).expect("buffer size")
    }

    pub fn to_rgba8_bytes(&self) -> Vec<u8> {
        self.px
            .par_iter()
            .flat_map_iter(|c| {
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                [q(c[0]), q(c[1]), q(c[2]), 255]
            })
            .collect()
    }

    #[inline]
    pub fn get(&self, x: isize, y: isize) -> Rgb {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.px[y * self.w + x]
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> Rgb {
        self.px[y * self.w + x]
    }

    /// Bilinear sample with clamped edges.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> Rgb {
        sample_bilinear(&self.px, self.w, self.h, x, y)
    }

    pub fn long_side(&self) -> usize {
        self.w.max(self.h)
    }

    /// High quality downscale so the long side is at most `long`.
    pub fn fit_long(&self, long: usize) -> Img {
        if self.long_side() <= long {
            return self.clone();
        }
        let s = long as f32 / self.long_side() as f32;
        let w = ((self.w as f32 * s).round() as u32).max(1);
        let h = ((self.h as f32 * s).round() as u32).max(1);
        let small = image::imageops::resize(&self.to_rgb8(), w, h, image::imageops::FilterType::Lanczos3);
        Img::from_rgb8(&small)
    }

    pub fn resize_exact(&self, w: usize, h: usize, filter: image::imageops::FilterType) -> Img {
        let r = image::imageops::resize(&self.to_rgb8(), w as u32, h as u32, filter);
        Img::from_rgb8(&r)
    }

    pub fn luma(&self) -> Vec<f32> {
        self.px.par_iter().map(|&c| luma(c)).collect()
    }

    pub fn map(&self, f: impl Fn(Rgb) -> Rgb + Sync) -> Img {
        Img { w: self.w, h: self.h, px: self.px.par_iter().map(|&c| f(c)).collect() }
    }

    pub fn map_xy(&self, f: impl Fn(usize, usize, Rgb) -> Rgb + Sync) -> Img {
        let w = self.w;
        let px = self.px.par_iter().enumerate().map(|(i, &c)| f(i % w, i / w, c)).collect();
        Img { w: self.w, h: self.h, px }
    }
}

// ---------------------------------------------------------------- colour math

#[inline]
pub fn luma(c: Rgb) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

#[inline]
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[inline]
pub fn mul(a: Rgb, b: Rgb) -> Rgb {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

#[inline]
pub fn scale(a: Rgb, s: f32) -> Rgb {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub fn add(a: Rgb, b: Rgb) -> Rgb {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub fn clamp01(a: Rgb) -> Rgb {
    [a[0].clamp(0.0, 1.0), a[1].clamp(0.0, 1.0), a[2].clamp(0.0, 1.0)]
}

#[inline]
pub fn dist2(a: Rgb, b: Rgb) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn saturate(c: Rgb, amount: f32) -> Rgb {
    let l = luma(c);
    clamp01([l + (c[0] - l) * amount, l + (c[1] - l) * amount, l + (c[2] - l) * amount])
}

pub fn rgb_to_hsv(c: Rgb) -> Rgb {
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 1e-6 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    let s = if max <= 1e-6 { 0.0 } else { d / max };
    [h, s, max]
}

pub fn hsv_to_rgb(c: Rgb) -> Rgb {
    let (h, s, v) = (c[0].rem_euclid(1.0) * 6.0, c[1].clamp(0.0, 1.0), c[2]);
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

pub fn hex(c: u32) -> Rgb {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

// ---------------------------------------------------------------- generic pixels

pub trait Px: Copy + Send + Sync + Default {
    fn add(self, o: Self) -> Self;
    fn sub(self, o: Self) -> Self;
    fn scale(self, s: f32) -> Self;
}

impl Px for f32 {
    #[inline]
    fn add(self, o: Self) -> Self {
        self + o
    }
    #[inline]
    fn sub(self, o: Self) -> Self {
        self - o
    }
    #[inline]
    fn scale(self, s: f32) -> Self {
        self * s
    }
}

impl Px for Rgb {
    #[inline]
    fn add(self, o: Self) -> Self {
        [self[0] + o[0], self[1] + o[1], self[2] + o[2]]
    }
    #[inline]
    fn sub(self, o: Self) -> Self {
        [self[0] - o[0], self[1] - o[1], self[2] - o[2]]
    }
    #[inline]
    fn scale(self, s: f32) -> Self {
        [self[0] * s, self[1] * s, self[2] * s]
    }
}

impl Px for [f32; 2] {
    #[inline]
    fn add(self, o: Self) -> Self {
        [self[0] + o[0], self[1] + o[1]]
    }
    #[inline]
    fn sub(self, o: Self) -> Self {
        [self[0] - o[0], self[1] - o[1]]
    }
    #[inline]
    fn scale(self, s: f32) -> Self {
        [self[0] * s, self[1] * s]
    }
}

#[inline]
pub fn sample_bilinear<T: Px>(buf: &[T], w: usize, h: usize, x: f32, y: f32) -> T {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let a = buf[y0 * w + x0].scale((1.0 - fx) * (1.0 - fy));
    let b = buf[y0 * w + x1].scale(fx * (1.0 - fy));
    let c = buf[y1 * w + x0].scale((1.0 - fx) * fy);
    let d = buf[y1 * w + x1].scale(fx * fy);
    a.add(b).add(c).add(d)
}

fn transpose<T: Px>(src: &[T], w: usize, h: usize) -> Vec<T> {
    let mut out = vec![T::default(); w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, o) in col.iter_mut().enumerate() {
            *o = src[y * w + x];
        }
    });
    out
}

fn box_rows<T: Px>(src: &[T], w: usize, r: usize) -> Vec<T> {
    let mut out = vec![T::default(); src.len()];
    let norm = 1.0 / (2 * r + 1) as f32;
    let wi = w as isize;
    out.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(row, s)| {
        let at = |i: isize| s[i.clamp(0, wi - 1) as usize];
        let ri = r as isize;
        let mut acc = T::default();
        for i in -ri..=ri {
            acc = acc.add(at(i));
        }
        for x in 0..wi {
            row[x as usize] = acc.scale(norm);
            acc = acc.add(at(x + ri + 1)).sub(at(x - ri));
        }
    });
    out
}

fn gauss_rows<T: Px>(src: &[T], w: usize, kernel: &[f32]) -> Vec<T> {
    let mut out = vec![T::default(); src.len()];
    let r = (kernel.len() / 2) as isize;
    let wi = w as isize;
    out.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(row, s)| {
        for x in 0..wi {
            let mut acc = T::default();
            for (k, &kw) in kernel.iter().enumerate() {
                let i = (x + k as isize - r).clamp(0, wi - 1) as usize;
                acc = acc.add(s[i].scale(kw));
            }
            row[x as usize] = acc;
        }
    });
    out
}

fn gauss_kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as isize;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    k
}

/// Gaussian blur. Exact separable kernel for small sigma, triple box
/// (running sums, O(1) per pixel) for large sigma.
pub fn blur<T: Px>(src: &[T], w: usize, h: usize, sigma: f32) -> Vec<T> {
    if sigma < 0.3 {
        return src.to_vec();
    }
    if sigma < 4.0 {
        let k = gauss_kernel(sigma);
        let a = gauss_rows(src, w, &k);
        let t = transpose(&a, w, h);
        let b = gauss_rows(&t, h, &k);
        return transpose(&b, h, w);
    }
    let r = (((12.0 * sigma * sigma / 3.0 + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
    let mut a = src.to_vec();
    for _ in 0..3 {
        a = box_rows(&a, w, r);
    }
    let mut t = transpose(&a, w, h);
    for _ in 0..3 {
        t = box_rows(&t, h, r);
    }
    transpose(&t, h, w)
}

pub fn blur_img(img: &Img, sigma: f32) -> Img {
    Img { w: img.w, h: img.h, px: blur(&img.px, img.w, img.h, sigma) }
}

// ---------------------------------------------------------------- flow fields

/// Per pixel: (tangent x, tangent y, anisotropy 0..1, gradient magnitude).
pub type Flow = [f32; 4];

/// Smoothed structure tensor -> edge tangent field (Kyprianidis et al.).
pub fn flow_field(img: &Img, sigma: f32) -> Vec<Flow> {
    let (w, h) = (img.w, img.h);
    let tensor: Vec<Rgb> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let mut e = 0.0;
            let mut f = 0.0;
            let mut g = 0.0;
            for c in 0..3 {
                let p = |dx: isize, dy: isize| img.get(x + dx, y + dy)[c];
                let gx = (p(1, -1) + 2.0 * p(1, 0) + p(1, 1) - p(-1, -1) - 2.0 * p(-1, 0) - p(-1, 1)) / 4.0;
                let gy = (p(-1, 1) + 2.0 * p(0, 1) + p(1, 1) - p(-1, -1) - 2.0 * p(0, -1) - p(1, -1)) / 4.0;
                e += gx * gx;
                f += gx * gy;
                g += gy * gy;
            }
            [e, f, g]
        })
        .collect();
    let tensor = blur(&tensor, w, h, sigma);
    tensor
        .par_iter()
        .map(|&[e, f, g]| {
            let d = ((e - g) * (e - g) + 4.0 * f * f).sqrt();
            let l1 = 0.5 * (e + g + d);
            let l2 = 0.5 * (e + g - d);
            let t1 = [l1 - e, -f];
            let t2 = [-f, l1 - g];
            let (n1, n2) = (t1[0].hypot(t1[1]), t2[0].hypot(t2[1]));
            let (t, n) = if n1 >= n2 { (t1, n1) } else { (t2, n2) };
            let t = if n > 1e-9 { [t[0] / n, t[1] / n] } else { [0.0, 1.0] };
            let a = if l1 + l2 > 1e-9 { (l1 - l2) / (l1 + l2) } else { 0.0 };
            [t[0], t[1], a, l1.max(0.0).sqrt()]
        })
        .collect()
}

#[inline]
pub fn flow_at(flow: &[Flow], w: usize, h: usize, x: f32, y: f32) -> [f32; 2] {
    let xi = (x.round() as isize).clamp(0, w as isize - 1) as usize;
    let yi = (y.round() as isize).clamp(0, h as isize - 1) as usize;
    let f = flow[yi * w + xi];
    [f[0], f[1]]
}

/// Line integral convolution of any pixel buffer along the tangent field.
pub fn lic<T: Px>(src: &[T], flow: &[Flow], w: usize, h: usize, length: f32) -> Vec<T> {
    let steps = length.max(1.0).round() as usize;
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x0, y0) = ((i % w) as f32, (i / w) as f32);
            let mut acc = src[i];
            let mut wsum = 1.0;
            for dir in [1.0f32, -1.0] {
                let (mut x, mut y) = (x0, y0);
                let t0 = flow_at(flow, w, h, x, y);
                let mut prev = [t0[0] * dir, t0[1] * dir];
                for s in 1..=steps {
                    let mut t = flow_at(flow, w, h, x, y);
                    if t[0] * prev[0] + t[1] * prev[1] < 0.0 {
                        t = [-t[0], -t[1]];
                    }
                    x += t[0];
                    y += t[1];
                    if x < 0.0 || y < 0.0 || x > (w - 1) as f32 || y > (h - 1) as f32 {
                        break;
                    }
                    prev = t;
                    let k = 1.0 - s as f32 / (steps as f32 + 1.0);
                    acc = acc.add(sample_bilinear(src, w, h, x, y).scale(k));
                    wsum += k;
                }
            }
            acc.scale(1.0 / wsum)
        })
        .collect()
}

/// Sobel gradient magnitude of a scalar field.
pub fn gradient_mag(l: &[f32], w: usize, h: usize) -> Vec<f32> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let p = |dx: isize, dy: isize| {
                let xx = (x + dx).clamp(0, w as isize - 1) as usize;
                let yy = (y + dy).clamp(0, h as isize - 1) as usize;
                l[yy * w + xx]
            };
            let gx = p(1, -1) + 2.0 * p(1, 0) + p(1, 1) - p(-1, -1) - 2.0 * p(-1, 0) - p(-1, 1);
            let gy = p(-1, 1) + 2.0 * p(0, 1) + p(1, 1) - p(-1, -1) - 2.0 * p(0, -1) - p(1, -1);
            (gx * gx + gy * gy).sqrt() * 0.25
        })
        .collect()
}

/// Two-pass chamfer distance (in pixels) to the nearest `true` cell.
pub fn distance_transform(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    const A: f32 = 1.0;
    const B: f32 = std::f32::consts::SQRT_2;
    let big = (w + h) as f32;
    let mut d: Vec<f32> = mask.iter().map(|&m| if m { 0.0 } else { big }).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1] + A);
            }
            if y > 0 {
                v = v.min(d[i - w] + A);
                if x > 0 {
                    v = v.min(d[i - w - 1] + B);
                }
                if x + 1 < w {
                    v = v.min(d[i - w + 1] + B);
                }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w {
                v = v.min(d[i + 1] + A);
            }
            if y + 1 < h {
                v = v.min(d[i + w] + A);
                if x + 1 < w {
                    v = v.min(d[i + w + 1] + B);
                }
                if x > 0 {
                    v = v.min(d[i + w - 1] + B);
                }
            }
            d[i] = v;
        }
    }
    d
}

/// Light a height field from the upper left: returns a per-pixel shade
/// multiplier around 1.0 plus a specular term.
pub fn relief(height: &[f32], w: usize, h: usize, strength: f32) -> Vec<[f32; 2]> {
    let l = {
        let v = [-0.55f32, -0.65, 0.52];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    };
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let hx = |xx: usize| height[y * w + xx];
            let hy = |yy: usize| height[yy * w + x];
            let dx = hx((x + 1).min(w - 1)) - hx(x.saturating_sub(1));
            let dy = hy((y + 1).min(h - 1)) - hy(y.saturating_sub(1));
            let k = strength * 6.0;
            let n = [-dx * k, -dy * k, 1.0];
            let nl = (n[0] * n[0] + n[1] * n[1] + 1.0).sqrt();
            let ndl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]) / nl;
            let flat = l[2];
            let shade = 1.0 + (ndl - flat) * 1.1;
            // Blinn-ish highlight with view straight on.
            let hv = [l[0], l[1], l[2] + 1.0];
            let hn = (hv[0] * hv[0] + hv[1] * hv[1] + hv[2] * hv[2]).sqrt();
            let ndh = ((n[0] * hv[0] + n[1] * hv[1] + n[2] * hv[2]) / (nl * hn)).max(0.0);
            let spec = ndh.powf(40.0) * strength * 0.35;
            [shade, spec]
        })
        .collect()
}

pub fn apply_relief(img: &Img, height: &[f32], strength: f32) -> Img {
    if strength <= 0.001 {
        return img.clone();
    }
    let r = relief(height, img.w, img.h, strength);
    let px = img.px.par_iter().zip(r.par_iter()).map(|(&c, &[s, sp])| clamp01([c[0] * s + sp, c[1] * s + sp, c[2] * s + sp])).collect();
    Img { w: img.w, h: img.h, px }
}

// ---------------------------------------------------------------- noise & rng

#[inline]
pub fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

#[inline]
pub fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

/// Fractal noise in ~[0, 1].
pub fn fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let (mut amp, mut freq, mut sum, mut norm) = (0.5, 1.0, 0.0, 0.0);
    for o in 0..octaves {
        sum += amp * value_noise(x * freq, y * freq, seed.wrapping_add(o * 1013));
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm
}

/// SplitMix64 — tiny, deterministic, good enough for art.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f32()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

/// Soft anti-aliased disc stamp (used by brush, pointillism, splatter).
pub fn stamp_disc(img: &mut Img, cx: f32, cy: f32, r: f32, color: Rgb, alpha: f32) {
    let x0 = (cx - r - 1.0).floor().max(0.0) as usize;
    let y0 = (cy - r - 1.0).floor().max(0.0) as usize;
    let x1 = ((cx + r + 1.0).ceil() as usize).min(img.w.saturating_sub(1));
    let y1 = ((cy + r + 1.0).ceil() as usize).min(img.h.saturating_sub(1));
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let a = (r + 0.5 - d).clamp(0.0, 1.0) * alpha;
            if a > 0.0 {
                let i = y * img.w + x;
                img.px[i] = mix(img.px[i], color, a);
            }
        }
    }
}
