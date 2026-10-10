//! External plug-ins (BL-001): discovery in several folders (D9), approval
//! pinned to a SHA-256 checksum (D7), the manifest that describes a plug-in
//! (D4) and the one-process-per-render contract with files in a job folder
//! (D2, D5).
//!
//! The contract, as seen by a plug-in:
//!
//! * `plugin.toml` (a [`Manifest`]) describes it completely. The app reads it
//!   without running anything.
//! * `<command> render <job>/request.json` reads the [`RenderRequest`], writes
//!   a PNG to `output`, may print JSON Lines such as `{"progress": 0.5}`,
//!   `{"log": "…"}` or `{"error": "…"}`, and exits 0 on success.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use photo_airt_sdk::{Ctx, Params, REFERENCE_LONG_SIDE};
pub use photo_airt_sdk::{Manifest, PROTOCOL, RenderRequest, write_png};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Description, RenderError, StylePlugin};
use crate::imaging::Img;
use crate::photo_io;
const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 600;
/// Plug-in folders larger than this are refused (they are hashed in full).
const MAX_FOLDER_BYTES: u64 = 512 * 1024 * 1024;
/// Seeds are kept within JSON's exactly representable integer range.
const SEED_MASK: u64 = (1 << 53) - 1;

// ------------------------------------------------------------------ contract messages

/// One JSON Lines message a plug-in may print while rendering.
#[derive(Debug, Default, Deserialize)]
struct Line {
    progress: Option<f32>,
    error: Option<String>,
}

// ------------------------------------------------------------------ locations

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocationKind {
    /// A folder listed in `PHOTO_AIRT_PLUGINS`.
    Env,
    /// The user's folder in the OS data directory.
    User,
    /// `plugins/` next to the executable.
    Portable,
}

impl LocationKind {
    pub fn label(self) -> &'static str {
        match self {
            LocationKind::Env => "PHOTO_AIRT_PLUGINS",
            LocationKind::User => "Your plug-ins folder",
            LocationKind::Portable => "Next to the app",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Location {
    pub kind: LocationKind,
    pub path: PathBuf,
    pub exists: bool,
}

/// The user's plug-ins folder: `<OS data dir>/photo-airt/plugins`.
pub fn user_folder() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("photo-airt").join("plugins"))
}

/// The portable folder next to the real (symlink-resolved) executable.
pub fn portable_folder() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = photo_io::canonicalize(&exe).unwrap_or(exe);
    exe.parent().map(|p| p.join("plugins"))
}

/// Every place plug-ins are looked for, in precedence order (D9).
pub fn locations() -> Vec<Location> {
    let mut out = vec![];
    if let Some(list) = std::env::var_os("PHOTO_AIRT_PLUGINS") {
        for path in std::env::split_paths(&list).filter(|p| !p.as_os_str().is_empty()) {
            out.push(Location { kind: LocationKind::Env, exists: path.is_dir(), path });
        }
    }
    for (kind, path) in [(LocationKind::User, user_folder()), (LocationKind::Portable, portable_folder())] {
        if let Some(path) = path
            && !out.iter().any(|l| l.path == path)
        {
            out.push(Location { kind, exists: path.is_dir(), path });
        }
    }
    out
}

// ------------------------------------------------------------------ trust (D7)

/// Approved plug-in folders and the checksum each approval is pinned to.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Trust {
    #[serde(default)]
    pub approved: BTreeMap<String, String>,
}

impl Trust {
    fn path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("photo-airt").join("plugin-trust.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, serde_json::to_string_pretty(self).unwrap_or_default())
    }

    fn key(dir: &Path) -> String {
        dir.display().to_string()
    }

    /// The same folder as Photo·AIrt 0.2.0 recorded it on Windows, with the
    /// `\\?\` verbatim prefix, so those approvals stay valid.
    fn legacy_key(dir: &Path) -> Option<String> {
        let key = Self::key(dir);
        if key.starts_with(r"\\?\") {
            return None;
        }
        if let Some(share) = key.strip_prefix(r"\\") {
            return Some(format!(r"\\?\UNC\{share}"));
        }
        let drive = key.as_bytes();
        (drive.len() >= 3 && drive[0].is_ascii_alphabetic() && drive[1] == b':' && drive[2] == b'\\').then(|| format!(r"\\?\{key}"))
    }

    pub fn approve(&mut self, dir: &Path, checksum: &str) {
        self.revoke(dir);
        self.approved.insert(Self::key(dir), checksum.to_string());
    }

    pub fn revoke(&mut self, dir: &Path) {
        self.approved.remove(&Self::key(dir));
        if let Some(legacy) = Self::legacy_key(dir) {
            self.approved.remove(&legacy);
        }
    }

    /// `None`: never approved. `Some(false)`: approved, but the plug-in has
    /// changed since. `Some(true)`: approved as it is now.
    pub fn verdict(&self, dir: &Path, checksum: &str) -> Option<bool> {
        let recorded = self.approved.get(&Self::key(dir)).or_else(|| Self::legacy_key(dir).and_then(|k| self.approved.get(&k)));
        recorded.map(|c| c == checksum)
    }
}

/// SHA-256 over every file in the plug-in folder (relative path, size and
/// content, in sorted order), so any change requires re-approval.
pub fn folder_checksum(dir: &Path) -> Result<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "__pycache__" {
                continue;
            }
            let ft = e.file_type()?;
            if ft.is_dir() {
                walk(root, &e.path(), out)?;
            } else if ft.is_file() {
                out.push(e.path().strip_prefix(root).unwrap_or(&e.path()).to_path_buf());
            }
        }
        Ok(())
    }
    let mut files = vec![];
    walk(dir, dir, &mut files).map_err(|e| format!("cannot read plug-in folder: {e}"))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    for rel in files {
        let bytes = std::fs::read(dir.join(&rel)).map_err(|e| format!("cannot read {}: {e}", rel.display()))?;
        total += bytes.len() as u64;
        if total > MAX_FOLDER_BYTES {
            return Err(format!("plug-in folder is larger than {} MB", MAX_FOLDER_BYTES / 1_048_576));
        }
        hasher.update(rel.to_string_lossy().replace('\\', "/").as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// ------------------------------------------------------------------ running a plug-in

/// Environment passed to plug-ins: only non-secret system variables needed
/// to find programs and temporary folders.
const ENV_ALLOW: [&str; 14] = [
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "HOME",
    "USERPROFILE",
    "TEMP",
    "TMP",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "APPDATA",
    "LOCALAPPDATA",
];

fn command_for(dir: &Path, command: &[String], args: &[&str]) -> Command {
    let program = PathBuf::from(&command[0]);
    let in_folder = dir.join(&program);
    let program = if !program.is_absolute() && in_folder.is_file() { in_folder } else { program };
    let mut cmd = Command::new(program);
    cmd.args(&command[1..]).args(args).current_dir(dir).env_clear();
    for key in ENV_ALLOW {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("PHOTO_AIRT_PLUGIN_PROTOCOL", PROTOCOL.to_string()).env("PYTHONIOENCODING", "utf-8");
    cmd
}

/// Run one transaction: stream stdout lines to `on_line`, enforce the
/// timeout and cancellation, and return stdout's lines and stderr's tail.
fn run(
    dir: &Path,
    command: &[String],
    args: &[&str],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    mut on_line: impl FnMut(&str),
) -> Result<(), RenderError> {
    let mut child = command_for(dir, command, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RenderError::Failed(format!("could not start `{}`: {e}", command[0])))?;
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().expect("piped");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stderr = child.stderr.take().expect("piped");
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            on_line(&line);
        }
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RenderError::Cancelled);
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RenderError::Failed(format!("timed out after {} s", timeout.as_secs())));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(RenderError::Failed(format!("lost the plug-in process: {e}"))),
        }
    };
    // Drain whatever was printed just before exiting.
    while let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
        on_line(&line);
    }
    let stderr = err_thread.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        let tail: String = stderr.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ⏎ ");
        let tail = if tail.is_empty() { String::new() } else { format!(": {}", tail.chars().take(400).collect::<String>()) };
        Err(RenderError::Failed(format!("exited with {status}{tail}")))
    }
}

/// How long one render may take: what the plug-in asked for, within limits.
fn timeout(manifest: &Manifest) -> Duration {
    Duration::from_secs(manifest.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT))
}

/// A private job folder, removed on drop whatever the outcome (D5).
struct JobDir(PathBuf);

impl JobDir {
    fn new() -> std::io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = photo_io::cache_dir().join("plugin-jobs").join(format!("{}-{n}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for JobDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An approved external plug-in, behind [`StylePlugin`].
pub struct ProcessPlugin {
    dir: PathBuf,
    manifest: Manifest,
    desc: Description,
}

impl ProcessPlugin {
    pub fn new(dir: PathBuf, manifest: Manifest) -> Self {
        Self { desc: manifest.description(), dir, manifest }
    }
}

impl StylePlugin for ProcessPlugin {
    fn description(&self) -> &Description {
        &self.desc
    }

    fn folder(&self) -> Option<&Path> {
        Some(&self.dir)
    }

    fn render(&self, img: &Img, params: &Params, ctx: &Ctx) -> Result<Img, RenderError> {
        let fail = |what: &str, e: &dyn std::fmt::Display| RenderError::Failed(format!("{what}: {e}"));
        let job = JobDir::new().map_err(|e| fail("cannot create the job folder", &e))?;
        let (input, output, request_path) = (job.0.join("input.png"), job.0.join("output.png"), job.0.join("request.json"));
        write_png(&img.to_rgb8(), &input).map_err(|e| fail("cannot write the input image", &e))?;
        let request = RenderRequest {
            protocol: PROTOCOL,
            plugin_id: self.desc.id.clone(),
            input,
            output: output.clone(),
            width: img.w as u32,
            height: img.h as u32,
            params: params.0.clone(),
            seed: ctx.seed & SEED_MASK,
            scale: img.long_side() as f32 / REFERENCE_LONG_SIDE,
        };
        let json = serde_json::to_string_pretty(&request).map_err(|e| fail("cannot encode the request", &e))?;
        std::fs::write(&request_path, json).map_err(|e| fail("cannot write the request", &e))?;

        let mut reported: Option<String> = None;
        let request_arg = request_path.to_string_lossy().to_string();
        let result =
            run(&self.dir, self.manifest.command(), &["render", &request_arg], timeout(&self.manifest), Some(&ctx.cancel), |line| {
                if let Ok(msg) = serde_json::from_str::<Line>(line) {
                    if let Some(p) = msg.progress {
                        ctx.progress(p);
                    }
                    if let Some(e) = msg.error {
                        reported = Some(e);
                    }
                }
            });
        match (result, reported) {
            (Err(RenderError::Cancelled), _) => return Err(RenderError::Cancelled),
            (_, Some(e)) => return Err(RenderError::Failed(e)),
            (Err(e), None) => return Err(e),
            (Ok(()), None) => {}
        }
        // Decode with the same hardened decoder used for photos.
        let rgb = photo_io::load_photo(&output).map_err(|e| fail("the plug-in did not write a valid PNG", &e))?;
        if (rgb.width() as usize, rgb.height() as usize) != (img.w, img.h) {
            return Err(RenderError::Failed(format!("returned a {}×{} image for a {}×{} input", rgb.width(), rgb.height(), img.w, img.h)));
        }
        Ok(Img::from_rgb8(&rgb))
    }
}

// ------------------------------------------------------------------ discovery (D9)

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Approved as it is now, and available in the gallery.
    Ready,
    /// Never approved: it is never run.
    NeedsApproval,
    /// Approved earlier, but its files changed since: it is never run.
    Changed,
    /// Same id as a plug-in in a higher-precedence location.
    Shadowed(PathBuf),
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Found {
    pub dir: PathBuf,
    pub location: LocationKind,
    /// The manifest's name, or the folder name if the manifest is unreadable.
    pub name: String,
    pub checksum: Option<String>,
    pub status: Status,
    /// The plug-in's description, when its manifest is valid.
    pub manifest: Option<Manifest>,
    /// Approved by the user as it is now.
    pub approved: bool,
}

/// Result of scanning every plug-in location.
#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub locations: Vec<Location>,
    pub found: Vec<Found>,
}

impl Scan {
    /// Ready plug-ins, as [`StylePlugin`]s.
    pub fn plugins(&self) -> Vec<std::sync::Arc<dyn StylePlugin>> {
        self.found
            .iter()
            .filter(|f| f.status == Status::Ready)
            .filter_map(|f| {
                let m = f.manifest.clone()?;
                Some(std::sync::Arc::new(ProcessPlugin::new(f.dir.clone(), m)) as std::sync::Arc<dyn StylePlugin>)
            })
            .collect()
    }

    pub fn needing_approval(&self) -> usize {
        self.found.iter().filter(|f| matches!(f.status, Status::NeedsApproval | Status::Changed)).count()
    }
}

/// Scan every location. Scanning only reads files: plug-ins run only to
/// render, and only once approved as they are now.
pub fn scan(builtin_ids: &[String], trust: &Trust) -> Scan {
    scan_locations(locations(), builtin_ids, trust)
}

pub fn scan_locations(locations: Vec<Location>, builtin_ids: &[String], trust: &Trust) -> Scan {
    let mut found: Vec<Found> = vec![];
    let mut claimed: BTreeMap<String, PathBuf> = BTreeMap::new();
    for loc in locations.iter().filter(|l| l.exists) {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&loc.path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join(Manifest::FILE).is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let dir = photo_io::canonicalize(&dir).unwrap_or(dir);
            let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let mut f = Found {
                dir: dir.clone(),
                location: loc.kind,
                name,
                checksum: None,
                status: Status::NeedsApproval,
                manifest: None,
                approved: false,
            };
            f.status = match (Manifest::load(&dir), folder_checksum(&dir)) {
                (Err(e), _) | (_, Err(e)) => Status::Error(e),
                (Ok(manifest), Ok(checksum)) => {
                    let id = manifest.id.clone();
                    f.name = manifest.name.clone();
                    f.manifest = Some(manifest);
                    f.approved = trust.verdict(&dir, &checksum) == Some(true);
                    f.checksum = Some(checksum.clone());
                    if builtin_ids.contains(&id) {
                        Status::Error(format!("id “{id}” is used by a built-in style"))
                    } else if let Some(by) = claimed.get(&id) {
                        Status::Shadowed(by.clone())
                    } else {
                        claimed.insert(id, dir.clone());
                        match trust.verdict(&dir, &checksum) {
                            None => Status::NeedsApproval,
                            Some(false) => Status::Changed,
                            Some(true) => Status::Ready,
                        }
                    }
                }
            };
            found.push(f);
        }
    }
    Scan { locations, found }
}

// ------------------------------------------------------------------ validator

/// `photo-airt --check-plugin <dir>`: check a plug-in against the contract
/// without approving it. Returns a report and whether everything passed.
pub fn check(dir: &Path) -> (Vec<(bool, String)>, bool) {
    let mut report: Vec<(bool, String)> = vec![];
    let ok = |cond: bool, msg: String, report: &mut Vec<(bool, String)>| {
        report.push((cond, msg));
        cond
    };
    let manifest = match Manifest::load(dir) {
        Ok(m) => {
            ok(true, format!("{}: “{}” ({}) v{}, {} settings", Manifest::FILE, m.name, m.id, m.version, m.settings.len()), &mut report);
            ok(true, format!("runs {:?}", m.command()), &mut report);
            m
        }
        Err(e) => {
            ok(false, e, &mut report);
            return (report, false);
        }
    };
    match folder_checksum(dir) {
        Ok(c) => ok(true, format!("checksum {}", &c[..16]), &mut report),
        Err(e) => ok(false, e, &mut report),
    };
    let plugin = ProcessPlugin::new(dir.to_path_buf(), manifest);
    let params = Params::defaults(plugin.description());
    let mut all = true;
    let mut renders = vec![];
    for (w, h) in [(320usize, 240usize), (160, 120)] {
        let img = test_image(w, h);
        let ctx = Ctx::for_image(&img, 7);
        let t = Instant::now();
        match plugin.render(&img, &params, &ctx) {
            Ok(out) => {
                all &= ok(true, format!("render {w}×{h}: ok in {} ms", t.elapsed().as_millis()), &mut report);
                renders.push((img, out));
            }
            Err(e) => all &= ok(false, format!("render {w}×{h}: {e}"), &mut report),
        }
    }
    if let Some((img, first)) = renders.first() {
        match plugin.render(img, &params, &Ctx::for_image(img, 7)) {
            Ok(again) => all &= ok(again.to_rgb8() == first.to_rgb8(), "deterministic: same seed, same picture".into(), &mut report),
            Err(e) => all &= ok(false, format!("second render failed: {e}"), &mut report),
        }
    }
    (report, all && renders.len() == 2)
}

/// A colourful gradient used by the validator and tests.
pub fn test_image(w: usize, h: usize) -> Img {
    let mut img = Img::new(w, h, [0.0; 3]);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            img.px[y * w + x] = [u, v, ((u * 12.0).sin() * 0.5 + 0.5) * (1.0 - v)];
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use photo_airt_sdk::Value;

    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("photo-airt-plugin-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A valid manifest whose command would fail if it were ever run.
    const NEVER_RUNS: &str = "protocol = 1\nid = \"p\"\nname = \"P\"\nrun = [\"definitely-not-a-real-program-xyz\"]\n";

    #[test]
    fn approvals_recorded_with_windows_verbatim_paths_still_count() {
        let mut trust: Trust =
            serde_json::from_str(r#"{"approved": {"\\\\?\\C:\\Users\\me\\plugins\\p": "abc", "\\\\?\\UNC\\nas\\plugins\\q": "def"}}"#)
                .unwrap();
        let (local, share) = (Path::new(r"C:\Users\me\plugins\p"), Path::new(r"\\nas\plugins\q"));
        assert_eq!(trust.verdict(local, "abc"), Some(true));
        assert_eq!(trust.verdict(local, "changed"), Some(false));
        assert_eq!(trust.verdict(share, "def"), Some(true));
        assert_eq!(trust.verdict(Path::new("/home/me/plugins/p"), "abc"), None);
        // Approving again replaces the old entry rather than adding a second one.
        trust.approve(local, "new");
        assert_eq!(trust.approved.len(), 2);
        assert_eq!(trust.approved.get(r"C:\Users\me\plugins\p").map(String::as_str), Some("new"));
        trust.revoke(share);
        assert_eq!(trust.verdict(share, "def"), None);
    }

    #[test]
    fn checksum_changes_with_any_file_and_ignores_caches() {
        let d = tmp("checksum");
        std::fs::write(d.join("plugin.toml"), NEVER_RUNS).unwrap();
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::write(d.join("lib/x.py"), "print(1)").unwrap();
        let a = folder_checksum(&d).unwrap();
        std::fs::create_dir_all(d.join("__pycache__")).unwrap();
        std::fs::write(d.join("__pycache__/x.pyc"), "junk").unwrap();
        assert_eq!(folder_checksum(&d).unwrap(), a);
        std::fs::write(d.join("lib/x.py"), "print(2)").unwrap();
        assert_ne!(folder_checksum(&d).unwrap(), a);
    }

    #[test]
    fn unapproved_or_changed_plugins_are_never_run() {
        let root = tmp("trust");
        let p = root.join("p");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("plugin.toml"), NEVER_RUNS).unwrap();
        let loc = vec![Location { kind: LocationKind::Env, path: root.clone(), exists: true }];
        let s = scan_locations(loc.clone(), &[], &Trust::default());
        assert_eq!(s.found[0].status, Status::NeedsApproval);
        let mut trust = Trust::default();
        trust.approve(&s.found[0].dir, "stale-checksum");
        let s = scan_locations(loc, &[], &trust);
        assert_eq!(s.found[0].status, Status::Changed);
        assert!(s.plugins().is_empty());
    }

    #[test]
    fn approved_plugin_that_cannot_start_fails_to_render() {
        let root = tmp("broken");
        let p = root.join("p");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("plugin.toml"), NEVER_RUNS).unwrap();
        let dir = photo_io::canonicalize(&p).unwrap();
        let mut trust = Trust::default();
        trust.approve(&dir, &folder_checksum(&dir).unwrap());
        let s = scan_locations(vec![Location { kind: LocationKind::User, path: root, exists: true }], &[], &trust);
        assert_eq!(s.found[0].status, Status::Ready);
        let plugin = &s.plugins()[0];
        let img = test_image(16, 12);
        let err = plugin.render(&img, &Params::defaults(plugin.description()), &Ctx::for_image(&img, 1)).err();
        assert!(matches!(&err, Some(RenderError::Failed(e)) if e.contains("could not start")), "{err:?}");
    }

    #[test]
    fn invalid_manifests_and_clashing_ids_are_errors() {
        let root = tmp("clash");
        for (name, text) in [
            ("a-builtin", NEVER_RUNS.replace("\"p\"", "\"oil\"")),
            ("b-broken", "name = \"Broken\"\nrun = [\"x\"]\n".into()),
            ("c-first", NEVER_RUNS.into()),
            ("d-second", NEVER_RUNS.into()),
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("plugin.toml"), text).unwrap();
        }
        let s = scan_locations(vec![Location { kind: LocationKind::Env, path: root, exists: true }], &["oil".into()], &Trust::default());
        assert!(matches!(&s.found[0].status, Status::Error(e) if e.contains("built-in")), "{:?}", s.found[0].status);
        assert!(matches!(&s.found[1].status, Status::Error(e) if e.contains("plugin.toml")), "{:?}", s.found[1].status);
        assert_eq!(s.found[2].status, Status::NeedsApproval);
        assert!(matches!(&s.found[3].status, Status::Shadowed(by) if by == &s.found[2].dir));
    }

    /// The example plug-ins shipped in `examples/plugins` load through the
    /// real discovery, approval and registry path, and render through it.
    #[test]
    fn repository_example_plugins_load_into_the_registry() {
        let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/plugins");
        let locations = vec![Location { kind: LocationKind::Env, path: examples.clone(), exists: true }];
        let builtin = crate::plugins::Registry::builtin_ids();

        // Nothing runs before approval.
        let scan = scan_locations(locations.clone(), &builtin, &Trust::default());
        let names: Vec<&str> = scan.found.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Retro Print") && names.contains(&"Copperplate Engraving"), "{names:?}");
        assert!(scan.found.iter().all(|f| f.status == Status::NeedsApproval));
        assert_eq!(crate::plugins::Registry::with_external(&scan).all().len(), builtin.len());

        // Approve them as they are now.
        let mut trust = Trust::default();
        for f in &scan.found {
            trust.approve(&f.dir, f.checksum.as_deref().unwrap());
        }
        let scan = scan_locations(locations, &builtin, &trust);
        let registry = crate::plugins::Registry::with_external(&scan);
        for (id, interpreter) in [("retro-print", "python3"), ("copperplate", "node")] {
            let available = Command::new(if cfg!(windows) && interpreter == "python3" { "python" } else { interpreter })
                .arg("--version")
                .output()
                .is_ok();
            if !available {
                eprintln!("skipping {id}: {interpreter} is not installed");
                continue;
            }
            let plugin = registry.get(id).unwrap_or_else(|| panic!("{id} did not load: {:#?}", scan.found));
            assert!(plugin.folder().is_some(), "{id} should be external");
            assert!(registry.families().contains(&"Plug-in examples".to_string()));
            let img = test_image(96, 64);
            let out = plugin.render(&img, &Params::defaults(plugin.description()), &Ctx::for_image(&img, 3)).unwrap();
            assert_eq!((out.w, out.h), (96, 64));
        }
    }

    /// End to end with the example Python plug-in, when Python is available.
    #[test]
    fn example_python_plugin_round_trips() {
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/plugins/retro-print");
        let manifest = Manifest::load(&example).unwrap();
        if Command::new(&manifest.command()[0]).arg("--version").output().is_err() {
            eprintln!("skipping: {} is not installed", manifest.command()[0]);
            return;
        }
        let (report, ok) = check(&example);
        assert!(ok, "{report:#?}");
        // Settings of every kind reach the plug-in.
        let plugin = ProcessPlugin::new(example.clone(), manifest);
        let img = test_image(64, 48);
        let mut params = Params::defaults(plugin.description());
        params.0.insert("invert".into(), Value::Bool(true));
        let inverted = plugin.render(&img, &params, &Ctx::for_image(&img, 1)).unwrap();
        let normal = plugin.render(&img, &Params::defaults(plugin.description()), &Ctx::for_image(&img, 1)).unwrap();
        assert_ne!(inverted.to_rgb8(), normal.to_rgb8());
        // Cancellation stops the process.
        let ctx = Ctx::for_image(&img, 1);
        ctx.cancel.store(true, Ordering::Relaxed);
        assert_eq!(plugin.render(&img, &params, &ctx).err(), Some(RenderError::Cancelled));
    }
}
