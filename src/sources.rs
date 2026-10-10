//! Where photos come from: local files, links (an image or a web page with a
//! preview image) and the clipboard. Every source materialises its photo as a
//! local file so decoding, full-resolution export and the AI jobs work the
//! same no matter where the picture came from. No accounts, no setup.
//!
//! Downloads are deliberately conservative: https only, bounded size and
//! time, at most a handful of redirects, and the content must *sniff* as an
//! image (extensions and `Content-Type` are never trusted).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::photo_io;

/// Largest photo we will download.
pub const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;
/// Largest web page we will scan for a preview image.
const MAX_PAGE: u64 = 4 * 1024 * 1024;
const MAX_RECENTS: usize = 24;

// ------------------------------------------------------------------ origin & asset

/// Provenance of a photo, shown in the UI and on gallery wall labels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    File { path: PathBuf },
    Web { url: String, page: Option<String> },
    Clipboard,
}

impl Origin {
    pub fn icon(&self) -> &'static str {
        match self {
            Origin::File { .. } => "📁",
            Origin::Web { .. } => "🌐",
            Origin::Clipboard => "📋",
        }
    }

    /// Short label: "Local file", "en.wikipedia.org", "Clipboard".
    pub fn label(&self) -> String {
        match self {
            Origin::File { .. } => "Local file".into(),
            Origin::Web { url, page } => host(page.as_deref().unwrap_or(url)),
            Origin::Clipboard => "Clipboard".into(),
        }
    }

    /// Full location for tooltips.
    pub fn location(&self) -> String {
        match self {
            Origin::File { path } => path.display().to_string(),
            Origin::Web { url, page: Some(page) } => format!("{page}\nimage: {url}"),
            Origin::Web { url, page: None } => url.clone(),
            Origin::Clipboard => "Pasted from the clipboard".into(),
        }
    }

    /// Credit line for wall labels.
    pub fn credit(&self) -> String {
        match self {
            Origin::File { .. } => "From a local photograph".into(),
            Origin::Web { .. } => format!("From {}", self.label()),
            Origin::Clipboard => "Pasted from the clipboard".into(),
        }
    }

    /// Credit line for artworks made from this photo.
    pub fn after_credit(&self) -> String {
        match self {
            Origin::File { .. } => "After a photograph".into(),
            Origin::Web { .. } => format!("After a photograph from {}", self.label()),
            Origin::Clipboard => "After a pasted photograph".into(),
        }
    }

    /// Identity used to de-duplicate recents.
    fn key(&self, local: &Path) -> String {
        match self {
            Origin::File { path } => format!("file:{}", path.display()),
            Origin::Web { url, page } => format!("web:{}", page.as_deref().unwrap_or(url)),
            Origin::Clipboard => format!("clip:{}", local.display()),
        }
    }
}

fn host(url: &str) -> String {
    Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string())).unwrap_or_else(|| "the web".into())
}

/// A photo ready to decode: always a local file plus where it came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub local: PathBuf,
    pub name: String,
    pub origin: Origin,
}

pub fn local_asset(path: &Path) -> Result<Asset> {
    let path = crate::photo_io::canonicalize(path).with_context(|| format!("{} was not found", path.display()))?;
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    let name = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "Photo".into());
    Ok(Asset { local: path.clone(), name, origin: Origin::File { path } })
}

// ------------------------------------------------------------------ smart input

/// What the user typed, pasted or dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    File(PathBuf),
    Link(Url),
}

/// Understand a pasted string: a local path, an http(s) link, or a bare
/// `example.com/photo.jpg`. Existing relative paths take precedence over bare domains.
pub fn classify(raw: &str) -> Result<Input, String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let s = line.trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>');
    if s.is_empty() {
        return Err("Paste a link or a file path".into());
    }
    if s.starts_with("file://") {
        return Url::parse(s)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .map(Input::File)
            .ok_or_else(|| "That file:// link is not a valid path".into());
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return dirs::home_dir().map(|h| Input::File(h.join(rest))).ok_or_else(|| "No home directory".into());
    }
    let windows_drive = s.len() > 2 && s.as_bytes()[1] == b':' && (s.as_bytes()[2] == b'\\' || s.as_bytes()[2] == b'/');
    if s.starts_with('/')
        || windows_drive
        || s.starts_with("\\\\")
        || s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with(".\\")
        || s.starts_with("..\\")
    {
        return Ok(Input::File(PathBuf::from(s)));
    }
    if let Ok(u) = Url::parse(s) {
        return match u.scheme() {
            "http" | "https" => Ok(Input::Link(u)),
            other => Err(format!("{other}: links are not supported")),
        };
    }
    if Path::new(s).exists() {
        return Ok(Input::File(PathBuf::from(s)));
    }
    // Bare domain: "example.com/a.jpg".
    let first = s.split('/').next().unwrap_or("");
    let domain_like = first.contains('.')
        && !first.starts_with('.')
        && first.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':')
        && !s.contains(char::is_whitespace);
    if domain_like && let Ok(u) = Url::parse(&format!("https://{s}")) {
        return Ok(Input::Link(u));
    }
    if s.contains('/') || s.contains('\\') {
        return Ok(Input::File(PathBuf::from(s)));
    }
    Err("Not a link or a file path".into())
}

/// Every link or path in pasted text, one per line (invalid lines skipped).
pub fn classify_all(text: &str) -> Vec<Input> {
    text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| classify(l).ok()).collect()
}

/// Human description of a classified input for the smart bar.
pub fn describe(input: &Input) -> (String, String) {
    match input {
        Input::File(p) if p.is_file() => ("Local file".into(), p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()),
        Input::File(p) => ("File not found".into(), p.display().to_string()),
        Input::Link(u) => {
            let path = u.path().to_ascii_lowercase();
            let image_ext = [".jpg", ".jpeg", ".png", ".webp", ".gif", ".tif", ".tiff", ".bmp", ".heic", ".avif"];
            let what = if image_ext.iter().any(|e| path.ends_with(e)) {
                "Image link".to_string()
            } else {
                "Link: an image, or a page whose preview image will be used".to_string()
            };
            (what, host(u.as_str()))
        }
    }
}

// ------------------------------------------------------------------ downloads

#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: String,
    pub done: u64,
    pub total: Option<u64>,
}

pub struct Fetcher {
    agent: ureq::Agent,
    allow_http: bool,
    max_bytes: u64,
    dir: PathBuf,
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::with_policy(false, MAX_DOWNLOAD, photo_io::cache_dir().join("sources"))
    }
}

impl Fetcher {
    /// `allow_http` exists for tests against a local server; the app always
    /// uses https only.
    pub fn with_policy(allow_http: bool, max_bytes: u64, dir: PathBuf) -> Self {
        let agent = ureq::Agent::config_builder()
            .https_only(!allow_http)
            .http_status_as_error(false)
            .max_redirects(5)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(format!("Photo-AIrt/{} (+https://github.com/JCallico/photo-airt)", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent, allow_http, max_bytes, dir }
    }

    /// Fetch a link: either an image, or a web page whose preview image
    /// (`og:image`, `twitter:image`, `image_src`) is then downloaded.
    pub fn fetch(&self, url: &Url, report: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Asset> {
        self.check_scheme(url)?;
        let (bytes, mime) = self.get(url, self.max_bytes, &format!("Downloading from {}", host(url.as_str())), report, cancel)?;
        if let Some(ext) = sniff_image(&bytes) {
            let local = self.store(&bytes, ext, url.as_str())?;
            return Ok(Asset { local, name: name_from_url(url), origin: Origin::Web { url: url.to_string(), page: None } });
        }
        if !looks_like_html(&bytes, mime.as_deref()) {
            bail!("That link is not an image ({})", mime.unwrap_or_else(|| "unknown type".into()));
        }
        let page_len = bytes.len().min(MAX_PAGE as usize);
        let html = String::from_utf8_lossy(&bytes[..page_len]);
        let image = preview_image(&html, url).ok_or_else(|| anyhow!("That page has no preview image to open"))?;
        self.check_scheme(&image)?;
        let title = page_title(&html);
        let (img, _) = self.get(&image, self.max_bytes, "Downloading the page's preview image", report, cancel)?;
        let ext = sniff_image(&img).ok_or_else(|| anyhow!("The page's preview image is not a supported image"))?;
        let local = self.store(&img, ext, image.as_str())?;
        Ok(Asset {
            local,
            name: title.unwrap_or_else(|| name_from_url(&image)),
            origin: Origin::Web { url: image.to_string(), page: Some(url.to_string()) },
        })
    }

    fn check_scheme(&self, url: &Url) -> Result<()> {
        match url.scheme() {
            "https" => Ok(()),
            "http" if self.allow_http => Ok(()),
            "http" => bail!("Only secure https:// links are supported"),
            other => bail!("{other}: links are not supported"),
        }
    }

    fn get(
        &self,
        url: &Url,
        limit: u64,
        stage: &str,
        report: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<(Vec<u8>, Option<String>)> {
        report(Progress { stage: stage.to_string(), done: 0, total: None });
        let mut resp = self.agent.get(url.as_str()).call().map_err(|e| anyhow!("Could not reach {}: {e}", host(url.as_str())))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("{} answered {}", host(url.as_str()), status);
        }
        let total = resp.body().content_length();
        if total.is_some_and(|t| t > limit) {
            bail!("That file is too large ({} MB; the limit is {} MB)", total.unwrap() / 1_048_576, limit / 1_048_576);
        }
        let mime = resp.body().mime_type().map(str::to_string);
        let mut reader = resp.body_mut().with_config().limit(limit + 1).reader();
        let mut buf = Vec::with_capacity(total.unwrap_or(256 * 1024).min(limit) as usize);
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let n = match reader.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.to_string().contains("limit") => bail!("That file is larger than {} MB", limit / 1_048_576),
                Err(e) => return Err(anyhow!("Download interrupted: {e}")),
            };
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() as u64 > limit {
                bail!("That file is larger than {} MB", limit / 1_048_576);
            }
            report(Progress { stage: stage.to_string(), done: buf.len() as u64, total });
        }
        Ok((buf, mime))
    }

    fn store(&self, bytes: &[u8], ext: &str, key: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{:016x}.{ext}", fnv64(key.as_bytes()) ^ fnv64(&bytes[..bytes.len().min(4096)])));
        std::fs::write(&path, bytes).with_context(|| format!("saving {}", path.display()))?;
        Ok(path)
    }
}

fn name_from_url(url: &Url) -> String {
    url.path_segments().and_then(|mut s| s.next_back().map(percent_decode)).filter(|s| !s.is_empty()).unwrap_or_else(|| host(url.as_str()))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Identify supported image content from its first bytes. Returns the file
/// extension to store it under. HEIC/AVIF are recognised by their ISO-BMFF
/// brand and decoded through the external-converter fallback.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    use image::ImageFormat as F;
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return match &bytes[8..12] {
            b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" => Some("heic"),
            b"avif" | b"avis" => Some("avif"),
            _ => None,
        };
    }
    match image::guess_format(bytes).ok()? {
        F::Jpeg => Some("jpg"),
        F::Png => Some("png"),
        F::WebP => Some("webp"),
        F::Gif => Some("gif"),
        F::Tiff => Some("tiff"),
        F::Bmp => Some("bmp"),
        _ => None,
    }
}

fn looks_like_html(bytes: &[u8], mime: Option<&str>) -> bool {
    if mime.is_some_and(|m| m.contains("html")) {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html") || head.contains("<head")
}

// ------------------------------------------------------------------ html preview images

/// Attributes of every `<tag …>` in `html` (names lower-cased, values
/// entity-decoded). A tiny, forgiving scanner — enough for `<meta>`/`<link>`.
fn tags(html: &str, tag: &str) -> Vec<Vec<(String, String)>> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let mut out = vec![];
    let mut from = 0;
    while let Some(pos) = lower[from..].find(&open) {
        let start = from + pos + open.len();
        from = start;
        if !lower[start..].starts_with(|c: char| c.is_whitespace() || c == '/' || c == '>') {
            continue;
        }
        let Some(end) = lower[start..].find('>') else { break };
        out.push(attributes(&html[start..start + end]));
        from = start + end;
    }
    out
}

fn attributes(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = vec![];
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let ns = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        let name = s[ns..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let vs = i + 1;
                i = vs;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                value = s[vs..i].to_string();
                i += 1;
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = s[vs..i].to_string();
            }
        }
        if !name.is_empty() {
            out.push((name, decode_entities(&value)));
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&#x27;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// The best preview image a page advertises, resolved against `base`.
pub fn preview_image(html: &str, base: &Url) -> Option<Url> {
    const PRIORITY: [&str; 5] = ["og:image:secure_url", "og:image", "og:image:url", "twitter:image", "twitter:image:src"];
    let metas = tags(html, "meta");
    let mut best: Option<(usize, &str)> = None;
    for m in &metas {
        let key = attr(m, "property").or_else(|| attr(m, "name")).map(str::to_ascii_lowercase);
        let (Some(key), Some(content)) = (key, attr(m, "content")) else { continue };
        if let Some(rank) = PRIORITY.iter().position(|p| *p == key)
            && best.is_none_or(|(r, _)| rank < r)
            && !content.trim().is_empty()
        {
            best = Some((rank, content.trim()));
        }
    }
    let candidate = best.map(|(_, c)| c.to_string()).or_else(|| {
        tags(html, "link")
            .iter()
            .find(|l| attr(l, "rel").is_some_and(|r| r.eq_ignore_ascii_case("image_src")))
            .and_then(|l| attr(l, "href").map(str::to_string))
    })?;
    base.join(&candidate).ok().filter(|u| matches!(u.scheme(), "http" | "https"))
}

fn page_title(html: &str) -> Option<String> {
    let og = tags(html, "meta")
        .iter()
        .find(|m| attr(m, "property").is_some_and(|p| p.eq_ignore_ascii_case("og:title")))
        .and_then(|m| attr(m, "content").map(str::to_string));
    let title = og.or_else(|| {
        let lower = html.to_ascii_lowercase();
        let s = lower.find("<title")?;
        let s = s + lower[s..].find('>')? + 1;
        let e = s + lower[s..].find("</title")?;
        Some(decode_entities(&html[s..e]))
    })?;
    let t = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then(|| t.chars().take(80).collect())
}

// ------------------------------------------------------------------ clipboard

pub enum Clip {
    Image { width: usize, height: usize, rgba: Vec<u8> },
    Text(String),
    Empty,
}

pub fn read_clipboard() -> Result<Clip> {
    let mut cb = arboard::Clipboard::new().map_err(|e| anyhow!("clipboard unavailable: {e}"))?;
    if let Ok(img) = cb.get_image()
        && img.width > 0
        && img.height > 0
    {
        return Ok(Clip::Image { width: img.width, height: img.height, rgba: img.bytes.into_owned() });
    }
    match cb.get_text() {
        Ok(t) if !t.trim().is_empty() => Ok(Clip::Text(t)),
        _ => Ok(Clip::Empty),
    }
}

/// Save a pasted image as a PNG so it behaves like any other photo.
pub fn clipboard_asset(width: usize, height: usize, rgba: &[u8], dir: &Path) -> Result<Asset> {
    let img = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec()).ok_or_else(|| anyhow!("malformed clipboard image"))?;
    std::fs::create_dir_all(dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let local = dir.join(format!("clipboard-{stamp}.png"));
    image::DynamicImage::ImageRgba8(img).to_rgb8().save(&local).context("saving the pasted image")?;
    Ok(Asset { local, name: "Pasted image".into(), origin: Origin::Clipboard })
}

pub fn sources_dir() -> PathBuf {
    photo_io::cache_dir().join("sources")
}

/// Resolve any input to a local asset, blocking (command line and workers).
pub fn acquire(input: &Input, report: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Asset> {
    match input {
        Input::File(p) => local_asset(p),
        Input::Link(u) => Fetcher::default().fetch(u, report, cancel),
    }
}

// ------------------------------------------------------------------ recents

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recent {
    pub asset: Asset,
    pub thumb: PathBuf,
    pub opened: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Recents {
    pub items: Vec<Recent>,
}

impl Recents {
    fn path() -> PathBuf {
        photo_io::cache_dir().join("recents.json")
    }
    pub fn thumb_dir() -> PathBuf {
        photo_io::cache_dir().join("recents")
    }
    pub fn thumb_path(asset: &Asset) -> PathBuf {
        Self::thumb_dir().join(format!("{:016x}.jpg", fnv64(asset.origin.key(&asset.local).as_bytes())))
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn push(&mut self, asset: Asset, thumb: PathBuf) {
        let key = asset.origin.key(&asset.local);
        self.items.retain(|r| r.asset.origin.key(&r.asset.local) != key);
        let opened = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.items.insert(0, Recent { asset, thumb, opened });
        self.items.truncate(MAX_RECENTS);
    }

    pub fn remove(&mut self, local: &Path) {
        self.items.retain(|r| r.asset.local != local);
    }

    /// Persist, and delete cached downloads/thumbnails nothing refers to.
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(photo_io::cache_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
        let keep: Vec<&Path> = self.items.iter().flat_map(|r| [r.asset.local.as_path(), r.thumb.as_path()]).collect();
        for dir in [sources_dir(), Self::thumb_dir()] {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = entry.path();
                let old =
                    entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
                if old && !keep.contains(&p.as_path()) {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
    }

    /// The input to reopen a recent: its cached file if still there,
    /// otherwise the original location.
    pub fn reopen_input(r: &Recent) -> Result<Input, String> {
        if r.asset.local.is_file() {
            return Ok(Input::File(r.asset.local.clone()));
        }
        match &r.asset.origin {
            Origin::Web { url, page } => {
                Url::parse(page.as_deref().unwrap_or(url)).map(Input::Link).map_err(|_| "Invalid saved link".to_string())
            }
            Origin::File { path } => Err(format!("{} was moved or deleted", path.display())),
            Origin::Clipboard => Err("That pasted image is no longer cached".into()),
        }
    }
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn ago(secs: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(secs);
    let d = now.saturating_sub(secs);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86_400 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn classifies_paths_links_and_bare_domains() {
        assert_eq!(classify("/tmp/a.jpg"), Ok(Input::File("/tmp/a.jpg".into())));
        assert_eq!(classify("  \"/tmp/my photo.jpg\"\n"), Ok(Input::File("/tmp/my photo.jpg".into())));
        // file:// URLs are platform specific (Windows needs a drive letter), so build one for this OS.
        let local = std::env::temp_dir().join("a b.png");
        let url = Url::from_file_path(&local).unwrap();
        assert_eq!(classify(url.as_str()), Ok(Input::File(local)));
        assert!(matches!(classify("https://example.com/x.jpg"), Ok(Input::Link(u)) if u.host_str() == Some("example.com")));
        assert!(matches!(classify("example.com/photos/1.jpg"), Ok(Input::Link(u)) if u.scheme() == "https"));
        assert!(classify("ftp://example.com/a.jpg").is_err());
        assert!(classify("hello world").is_err());
        assert!(classify("   ").is_err());
    }

    #[test]
    fn classifies_every_line_of_a_multi_line_paste() {
        let text = "/tmp/a.jpg\n\n  not a link  \nhttps://example.com/b.png\nexample.org/c.webp\n";
        let all = classify_all(text);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], Input::File("/tmp/a.jpg".into()));
        assert!(matches!(&all[2], Input::Link(u) if u.as_str() == "https://example.org/c.webp"));
        assert!(classify_all("nothing useful here").is_empty());
    }

    #[test]
    fn classifies_relative_paths_even_when_missing() {
        for path in
            ["./photo.jpg", "../photo.jpg", "photos/photo.jpg", "photos/my photo.jpg", r".\photo.jpg", r"..\photo.jpg", r"photos\photo.jpg"]
        {
            assert_eq!(classify(path), Ok(Input::File(path.into())));
        }
        assert_eq!(classify("\"./my photo.jpg\""), Ok(Input::File("./my photo.jpg".into())));
        let all = classify_all("./photo.jpg\n../other.png\nhttps://example.com/photo.jpg");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], Input::File("./photo.jpg".into()));
        assert_eq!(all[1], Input::File("../other.png".into()));
        assert!(matches!(&all[2], Input::Link(_)));
    }

    #[test]
    fn existing_relative_files_take_precedence_over_bare_domains() {
        // Keep the process working directory unchanged so parallel tests stay isolated.
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let name = format!("photo-airt-relative-{}-{stamp}.jpg", std::process::id());
        let path = PathBuf::from(&name);
        std::fs::write(&path, b"local test file").unwrap();
        let input = classify(&name);
        let asset = local_asset(&path);
        let absolute = crate::photo_io::canonicalize(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(input, Ok(Input::File(path)));
        assert_eq!(asset.unwrap().local, absolute);
        // Without a matching local file, the same dotted name remains a bare domain.
        assert!(matches!(classify(&name), Ok(Input::Link(_))));
    }

    #[test]
    fn finds_the_best_preview_image() {
        let base = Url::parse("https://site.example/post/1").unwrap();
        let html = r#"<html><head>
            <meta name="twitter:image" content="https://cdn.example/tw.jpg">
            <META property='og:image' content="/img/og.jpg?a=1&amp;b=2" />
            <meta property="og:title" content="A  Sunny   Marsh">
            </head></html>"#;
        assert_eq!(preview_image(html, &base).unwrap().as_str(), "https://site.example/img/og.jpg?a=1&b=2");
        assert_eq!(page_title(html).as_deref(), Some("A Sunny Marsh"));
        let html = r#"<link rel="image_src" href="//cdn.example/p.png"><title>T</title>"#;
        assert_eq!(preview_image(html, &base).unwrap().as_str(), "https://cdn.example/p.png");
        assert!(preview_image("<meta property=\"og:image\" content=\"javascript:alert(1)\">", &base).is_none());
        assert!(preview_image("<p>no images</p>", &base).is_none());
    }

    #[test]
    fn sniffs_real_content_not_names() {
        let mut png = vec![];
        image::RgbImage::new(2, 2).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        assert_eq!(sniff_image(&png), Some("png"));
        assert_eq!(sniff_image(b"\0\0\0\x18ftypheic\0\0\0\0"), Some("heic"));
        assert_eq!(sniff_image(b"<!doctype html><html>"), None);
        assert_eq!(sniff_image(b"MZ\x90\0 not an image"), None);
    }

    /// Serve canned responses on 127.0.0.1 (one connection per response).
    fn serve(responses: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for (ctype, body) in responses {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let head =
                    format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        format!("http://{addr}")
    }

    fn png_bytes() -> Vec<u8> {
        let mut png = vec![];
        image::RgbImage::from_pixel(8, 6, image::Rgb([200, 100, 50]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("photo-airt-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn downloads_an_image_and_ignores_its_lying_content_type() {
        let base = serve(vec![("text/plain", png_bytes())]);
        let f = Fetcher::with_policy(true, MAX_DOWNLOAD, tmp_dir("img"));
        let url = Url::parse(&format!("{base}/photos/sunset.png")).unwrap();
        let mut steps = 0;
        let a = f.fetch(&url, &mut |_| steps += 1, &AtomicBool::new(false)).unwrap();
        assert_eq!(a.name, "sunset.png");
        assert_eq!(name_from_url(&Url::parse("https://x.example/a%20b.jpg").unwrap()), "a b.jpg");
        assert_eq!(a.local.extension().unwrap(), "png");
        assert!(steps >= 2);
        assert!(matches!(a.origin, Origin::Web { page: None, .. }));
        assert_eq!(image::open(&a.local).unwrap().width(), 8);
    }

    #[test]
    fn follows_a_page_to_its_preview_image() {
        // One server answers the page, then the image it points to.
        let base = serve(vec![
            (
                "text/html; charset=utf-8",
                b"<html><head><meta property=\"og:image\" content=\"/i.png\"><title>Marsh</title></head></html>".to_vec(),
            ),
            ("image/png", png_bytes()),
        ]);
        let f = Fetcher::with_policy(true, MAX_DOWNLOAD, tmp_dir("page"));
        let a = f.fetch(&Url::parse(&format!("{base}/post")).unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(a.name, "Marsh");
        assert!(matches!(&a.origin, Origin::Web { page: Some(p), url } if p.ends_with("/post") && url.ends_with("/i.png")));
    }

    #[test]
    fn rejects_http_non_images_and_oversized_downloads() {
        let strict = Fetcher::with_policy(false, MAX_DOWNLOAD, tmp_dir("strict"));
        let err = strict.fetch(&Url::parse("http://127.0.0.1:9/a.png").unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(err.to_string().contains("https"));

        let base = serve(vec![("application/octet-stream", b"MZ not an image at all".to_vec())]);
        let f = Fetcher::with_policy(true, MAX_DOWNLOAD, tmp_dir("bin"));
        let err = f.fetch(&Url::parse(&format!("{base}/x.png")).unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(err.to_string().contains("not an image"), "{err}");

        let base = serve(vec![("image/png", vec![0u8; 4096])]);
        let tiny = Fetcher::with_policy(true, 1024, tmp_dir("big"));
        let err = tiny.fetch(&Url::parse(&format!("{base}/big.png")).unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(err.to_string().contains("too large") || err.to_string().contains("larger"), "{err}");
    }

    #[test]
    fn cancellation_stops_a_download() {
        let base = serve(vec![("image/png", png_bytes())]);
        let f = Fetcher::with_policy(true, MAX_DOWNLOAD, tmp_dir("cancel"));
        let err = f.fetch(&Url::parse(&format!("{base}/a.png")).unwrap(), &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
        assert!(err.to_string().contains("cancelled"));
    }

    #[test]
    fn recents_dedupe_by_origin_and_cap() {
        let mut r = Recents::default();
        let a = |p: &str| Asset { local: p.into(), name: p.into(), origin: Origin::File { path: p.into() } };
        r.push(a("/a.jpg"), "/t1".into());
        r.push(a("/b.jpg"), "/t2".into());
        r.push(a("/a.jpg"), "/t3".into());
        assert_eq!(r.items.len(), 2);
        assert_eq!(r.items[0].asset.local, PathBuf::from("/a.jpg"));
        for i in 0..40 {
            r.push(a(&format!("/{i}.jpg")), "/t".into());
        }
        assert_eq!(r.items.len(), MAX_RECENTS);
    }
}
