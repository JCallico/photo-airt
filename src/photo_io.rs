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

/// `std::fs::canonicalize`, but without Windows' `\\?\` verbatim prefix when
/// the path means the same without it, so paths display and compare the way
/// users write them.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let path = std::fs::canonicalize(path)?;
    if cfg!(windows)
        && let Some(plain) = path.to_str().and_then(strip_verbatim)
    {
        return Ok(PathBuf::from(plain));
    }
    Ok(path)
}

/// `\\?\C:\dir` → `C:\dir` and `\\?\UNC\server\share` → `\\server\share`, but
/// only when the plain form is equivalent: shorter than `MAX_PATH`, and with
/// no reserved device names or trailing dots or spaces, which the verbatim
/// form allows and the plain form would reinterpret.
pub fn strip_verbatim(path: &str) -> Option<String> {
    let plain = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        let rest = path.strip_prefix(r"\\?\")?;
        let drive = rest.as_bytes();
        if !(drive.len() >= 3 && drive[0].is_ascii_alphabetic() && drive[1] == b':' && drive[2] == b'\\') {
            return None;
        }
        rest.to_string()
    };
    const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    let reserved = |c: &str| {
        let stem = c.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        RESERVED.contains(&stem.as_str())
            || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit())
    };
    let suspicious = plain.split('\\').filter(|c| !c.is_empty()).any(|c| reserved(c) || c.ends_with('.') || c.ends_with(' '));
    (plain.len() < 260 && !suspicious).then_some(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_removed_only_when_safe() {
        assert_eq!(strip_verbatim(r"\\?\C:\Users\me\photo.jpg").as_deref(), Some(r"C:\Users\me\photo.jpg"));
        assert_eq!(strip_verbatim(r"\\?\UNC\nas\photos\a.jpg").as_deref(), Some(r"\\nas\photos\a.jpg"));
        assert_eq!(strip_verbatim(r"C:\already\plain"), None);
        assert_eq!(strip_verbatim("/home/me/photo.jpg"), None);
        assert_eq!(strip_verbatim(r"\\?\Volume{1234}\dir"), None, "no drive letter");
        assert_eq!(strip_verbatim(r"\\?\C:\dir\CON"), None, "reserved device name");
        assert_eq!(strip_verbatim(r"\\?\C:\dir\com1.txt"), None, "reserved device name with extension");
        assert_eq!(strip_verbatim(r"\\?\C:\dir\name."), None, "trailing dot");
        assert_eq!(strip_verbatim(&format!(r"\\?\C:\{}", "a".repeat(300))), None, "longer than MAX_PATH");
        assert_eq!(strip_verbatim(r"\\?\C:\console\confirm.txt").as_deref(), Some(r"C:\console\confirm.txt"));
    }
}
