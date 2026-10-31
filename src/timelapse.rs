//! Painting time-lapses (BL-004): a looping GIF of an artwork being painted.
//!
//! The style is rendered again with a [`Ctx`] that records the snapshots it
//! takes at the ends of its passes. The animation shows the photo, then those
//! stages crossfading into one another, then holds the finished artwork.
//! Styles that take no snapshots get a brush sweep from the photo instead.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use photo_airt_sdk::imaging::Img;

use crate::finish::Finish;
use crate::plugins::{Ctx, Params, StylePlugin};

/// Long side of the animation, in pixels.
pub const LONG_SIDE: usize = 640;
/// Painting frames between the photo and the finished artwork, for the
/// brush sweep and for styles with few stages.
const PAINT_FRAMES: usize = 30;
/// Most stages shown; each gets at least two frames.
const MAX_STAGES: usize = 15;
const FRAME_MS: u32 = 90;
const PHOTO_HOLD_MS: u32 = 1200;
const FINAL_HOLD_MS: u32 = 2600;

/// One frame of the animation and how long it shows.
pub struct Still {
    pub img: Img,
    pub ms: u32,
}

/// Render `plugin` on `photo`, keeping every snapshot the style takes.
/// Returns the finished artwork and the snapshots, in order.
pub fn record(plugin: &dyn StylePlugin, photo: &Img, params: &Params, ctx: Ctx) -> Result<(Img, Vec<Img>)> {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let sink = frames.clone();
    let ctx = ctx.with_snapshots(Arc::new(move |img: &Img| sink.lock().unwrap().push(img.clone())));
    let out = plugin.render(photo, params, &ctx)?;
    drop(ctx);
    let frames = std::mem::take(&mut *frames.lock().unwrap());
    Ok((out, frames))
}

/// The animation: the photo, the painting, then the finished artwork.
/// `finish` is the artwork's finishing layer, applied to every painted frame.
pub fn compose(photo: &Img, snapshots: &[Img], artwork: &Img, finish: &Finish) -> Vec<Still> {
    let artwork = finish.apply(artwork);
    let mut stills = vec![Still { img: photo.clone(), ms: PHOTO_HOLD_MS }];
    // Keyframes: the stages a style painted, ending on the artwork. A style
    // without stages sweeps from the photo instead.
    let stages: Vec<Img> = pick_evenly(snapshots, MAX_STAGES).into_iter().map(|s| finish.apply(s)).collect();
    if stages.is_empty() {
        for k in 1..PAINT_FRAMES {
            stills.push(Still { img: sweep(photo, &artwork, k as f32 / PAINT_FRAMES as f32), ms: FRAME_MS });
        }
    } else {
        let keys: Vec<&Img> = std::iter::once(photo).chain(stages.iter()).chain(std::iter::once(&artwork)).collect();
        // Spread the frames over the steps between keyframes; a style with
        // few stages gets longer crossfades.
        let steps = keys.len() - 1;
        let per_step = PAINT_FRAMES.div_ceil(steps).max(2);
        for pair in keys.windows(2) {
            for k in 1..=per_step {
                let t = k as f32 / per_step as f32;
                if k == per_step && std::ptr::eq(pair[1], &artwork) {
                    break;
                }
                stills.push(Still { img: crossfade(pair[0], pair[1], t), ms: FRAME_MS });
            }
        }
    }
    stills.push(Still { img: artwork, ms: FINAL_HOLD_MS });
    stills
}

/// Up to `n` items spread evenly over `items`, always keeping the last one.
fn pick_evenly<T>(items: &[T], n: usize) -> Vec<&T> {
    if items.len() <= n {
        return items.iter().collect();
    }
    (1..=n).map(|k| &items[k * items.len() / n - 1]).collect()
}

fn crossfade(a: &Img, b: &Img, t: f32) -> Img {
    let mut out = a.clone();
    for (o, q) in out.px.iter_mut().zip(&b.px) {
        for c in 0..3 {
            o[c] += (q[c] - o[c]) * t;
        }
    }
    out
}

/// A soft diagonal brush sweep revealing `to` over `from`; `t` from 0 to 1.
fn sweep(from: &Img, to: &Img, t: f32) -> Img {
    let (w, h) = (from.w as f32, from.h as f32);
    let soft = 0.12;
    let edge = t * (1.0 + soft);
    let mut out = from.clone();
    for (i, (o, q)) in out.px.iter_mut().zip(&to.px).enumerate() {
        let (x, y) = ((i % from.w) as f32 / w, (i / from.w) as f32 / h);
        // Bristle streaks make the edge ragged, like a wet brush.
        let streak = ((y * 97.0).sin() * 0.5 + 0.5) * 0.05;
        let pos = (x * 0.75 + (1.0 - y) * 0.25) + streak;
        let a = ((edge - pos) / soft).clamp(0.0, 1.0);
        for c in 0..3 {
            o[c] += (q[c] - o[c]) * a;
        }
    }
    out
}

/// Write the animation as an infinitely looping GIF.
pub fn encode_gif(stills: &[Still], path: &Path) -> Result<()> {
    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 10);
    encoder.set_repeat(Repeat::Infinite)?;
    for s in stills {
        let rgb = s.img.to_rgb8();
        let rgba = RgbaImage::from_fn(rgb.width(), rgb.height(), |x, y| {
            let p = rgb.get_pixel(x, y);
            image::Rgba([p[0], p[1], p[2], 255])
        });
        encoder.encode_frame(Frame::from_parts(rgba, 0, 0, Delay::from_numer_denom_ms(s.ms, 1)))?;
    }
    Ok(())
}

/// Record, compose and save a time-lapse of `plugin` on `photo` (already at
/// the animation's size) to a new file in `dir` named after `stem`.
pub fn export(
    plugin: &dyn StylePlugin,
    photo: &Img,
    params: &Params,
    seed: u64,
    finish: &Finish,
    dir: &Path,
    stem: &str,
) -> Result<PathBuf> {
    let (artwork, snapshots) = record(plugin, photo, params, Ctx::for_image(photo, seed))?;
    let stills = compose(photo, &snapshots, &artwork, finish);
    let path = crate::photo_io::unique_path(dir, &format!("{stem}-timelapse"), "gif")?;
    encode_gif(&stills, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{self, Params};

    fn test_image(w: usize, h: usize) -> Img {
        let mut img = Img::new(w, h, [0.0; 3]);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                img.px[y * w + x] = [fx, fy, 0.5 + 0.5 * ((fx * 9.0).sin() * (fy * 7.0).cos())];
            }
        }
        img
    }

    /// Recording a time-lapse must never change the artwork.
    #[test]
    fn recording_leaves_every_builtin_style_unchanged() {
        let img = test_image(120, 80);
        for plugin in plugins::registry().all().iter().filter(|p| p.folder().is_none()) {
            let params = Params::defaults(plugin.description());
            let plain = plugin.render(&img, &params, &Ctx::for_image(&img, 5)).unwrap();
            let (recorded, snapshots) = record(plugin.as_ref(), &img, &params, Ctx::for_image(&img, 5)).unwrap();
            assert!(plain.px == recorded.px, "{} changed while recording", plugin.id());
            assert!(snapshots.iter().all(|s| (s.w, s.h) == (img.w, img.h)), "{} snapshot size", plugin.id());
        }
    }

    #[test]
    fn styles_with_passes_take_snapshots() {
        let img = test_image(120, 80);
        for id in ["brush", "watercolor", "stained_glass", "pencil", "lowpoly"] {
            let plugin = plugins::registry().get(id).cloned().unwrap();
            let (_, snapshots) = record(plugin.as_ref(), &img, &Params::defaults(plugin.description()), Ctx::for_image(&img, 5)).unwrap();
            assert!(!snapshots.is_empty(), "{id} took no snapshots");
        }
    }

    #[test]
    fn animation_starts_on_the_photo_and_ends_on_the_artwork() {
        let (photo, art) = (test_image(40, 30), Img::new(40, 30, [0.2, 0.4, 0.6]));
        for snapshots in [vec![], vec![Img::new(40, 30, [0.5; 3]); 3], vec![Img::new(40, 30, [0.5; 3]); 90]] {
            let stills = compose(&photo, &snapshots, &art, &Finish::default());
            assert!(stills.first().unwrap().img.px == photo.px);
            assert!(stills.last().unwrap().img.px == art.px);
            assert!((10..=2 * (MAX_STAGES + 1) + 2).contains(&stills.len()), "{} frames", stills.len());
        }
    }

    #[test]
    fn gif_loops_with_every_frame() {
        use image::AnimationDecoder;
        let photo = test_image(40, 30);
        let stills = compose(&photo, &[], &Img::new(40, 30, [0.9, 0.1, 0.1]), &Finish::default());
        let path = std::env::temp_dir().join(format!("photo-airt-timelapse-test-{}.gif", std::process::id()));
        encode_gif(&stills, &path).unwrap();
        let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap())).unwrap();
        let frames = decoder.into_frames().collect_frames().unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(frames.len(), stills.len());
        assert_eq!(frames[0].delay().numer_denom_ms(), (PHOTO_HOLD_MS, 1));
    }

    #[test]
    fn pick_evenly_keeps_the_last_item() {
        let v: Vec<u32> = (0..100).collect();
        let picked = pick_evenly(&v, 10);
        assert_eq!(picked.len(), 10);
        assert_eq!(*picked[9], 99);
        assert_eq!(pick_evenly(&v[..3], 10).len(), 3);
    }
}
