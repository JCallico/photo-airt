//! Loading photos (EXIF orientation aware, with an ImageMagick / libvips
//! fallback for HEIC and other formats the `image` crate cannot decode) and
//! saving artworks.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use image::{DynamicImage, ImageDecoder, ImageReader};

pub fn load_photo(path: &Path) -> Result<image::RgbImage> {
    match decode_native(path) {
        Ok(img) => Ok(img),
        Err(native_err) => decode_via_cli(path).map_err(|cli_err| {
            anyhow!("could not decode {}: {native_err:#}; external converters also failed: {cli_err:#}", path.display())
        }),
    }
}

/// Guard against decompression bombs: photos can come from the internet.
fn limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(30_000);
    l.max_image_height = Some(30_000);
    l.max_alloc = Some(1024 * 1024 * 1024);
    l
}

fn decode_native(path: &Path) -> Result<image::RgbImage> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(limits());
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img.to_rgb8())
}

fn decode_via_cli(path: &Path) -> Result<image::RgbImage> {
    let tmp = std::env::temp_dir().join(format!("photo-airt-convert-{}.png", std::process::id()));
    // Only hand converters files whose *content* is a known photo format, and
    // pin ImageMagick to that coder so it never guesses (or runs scripts).
    let mut head = Vec::with_capacity(64);
    if let Ok(f) = std::fs::File::open(path) {
        let _ = std::io::Read::read_to_end(&mut std::io::Read::take(f, 64), &mut head);
    }
    let coder = match crate::sources::sniff_image(&head) {
        Some(ext) => ext,
        None => bail!("{} is not a recognised image format", path.display()),
    };
    let attempts: [(&str, Vec<String>); 3] = [
        ("magick", vec![format!("{coder}:{}[0]", path.display()), "-auto-orient".into(), tmp.display().to_string()]),
        ("vips", vec!["autorot".into(), path.display().to_string(), tmp.display().to_string()]),
        ("heif-convert", vec![path.display().to_string(), tmp.display().to_string()]),
    ];
    let mut last = anyhow!("no converter available");
    for (bin, args) in attempts {
        match Command::new(bin).args(&args).output() {
            Ok(out) if out.status.success() && tmp.exists() => {
                let img = image::open(&tmp).context("reading converted image")?.to_rgb8();
                let _ = std::fs::remove_file(&tmp);
                return Ok(img);
            }
            Ok(out) => last = anyhow!("{bin}: {}", String::from_utf8_lossy(&out.stderr).trim()),
            Err(e) => last = anyhow!("{bin}: {e}"),
        }
    }
    Err(last)
}

pub fn output_dir() -> PathBuf {
    let base = dirs::picture_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir);
    base.join("Photo-AIrt")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("photo-airt")
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(60).collect()
}

/// Save without overwriting: `name.png`, `name-2.png`, ...
pub fn save_unique(img: &image::RgbImage, dir: &Path, stem: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stem = if stem.is_empty() { "artwork".to_string() } else { slug(stem) };
    let mut path = dir.join(format!("{stem}.png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}.png"));
        n += 1;
    }
    img.save(&path).with_context(|| format!("saving {}", path.display()))?;
    Ok(path)
}
