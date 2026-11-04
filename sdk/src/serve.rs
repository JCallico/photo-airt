//! The plug-in side of the external contract, `render <request.json>`, for
//! styles written in Rust.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::contract::{Ctx, Description, PROTOCOL, Params, RenderRequest, Style};
use crate::imaging::Img;

const USAGE: &str = "usage: <plug-in> render <request.json>";

/// The whole `main` of a style plug-in executable: answers `render` from the
/// command line.
pub fn serve(style: &Style) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match render_command(&args, &(style.description)(), |img, params, ctx| {
        (style.render)(img, params, ctx).ok_or_else(|| "cancelled".to_string())
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Answer `render <request.json>` (`args` excludes the program name) for a
/// style described by `desc` and rendered by `render`.
pub fn render_command(
    args: &[String],
    desc: &Description,
    render: impl FnOnce(&Img, &Params, &Ctx) -> Result<Img, String>,
) -> Result<(), String> {
    match args {
        [cmd, path] if cmd == "render" => {
            let result = render_request(Path::new(path), desc, render);
            if let Err(e) = &result {
                println!("{}", serde_json::json!({ "error": e }));
            }
            result
        }
        _ => Err(USAGE.into()),
    }
}

fn render_request(path: &Path, desc: &Description, render: impl FnOnce(&Img, &Params, &Ctx) -> Result<Img, String>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let req: RenderRequest = serde_json::from_str(&text).map_err(|e| format!("invalid request: {e}"))?;
    if req.protocol != PROTOCOL {
        return Err(format!("unsupported contract version {}", req.protocol));
    }
    let img = read_png(&req.input)?;
    let params = Params::sanitized(desc, &req.params);
    let mut ctx = Ctx::for_image(&img, req.seed);
    if let Some(dir) = req.snapshots.clone() {
        ctx = ctx.with_snapshots(snapshot_writer(dir));
    }

    // Report progress from a helper thread while rendering on this one.
    let done = Arc::new(AtomicBool::new(false));
    let reporter = {
        let (done, ctx) = (done.clone(), ctx.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                println!("{}", serde_json::json!({ "progress": ctx.progress_value() }));
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };
    let out = render(&img, &params, &ctx);
    done.store(true, Ordering::Relaxed);
    let _ = reporter.join();
    let out = out?;
    write_png(&out.to_rgb8(), &req.output).map_err(|e| format!("cannot write {}: {e}", req.output.display()))?;
    println!("{}", serde_json::json!({ "progress": 1.0 }));
    Ok(())
}

/// Writes each time-lapse snapshot to the request's folder and announces it.
fn snapshot_writer(dir: PathBuf) -> crate::contract::SnapshotSink {
    let count = Arc::new(AtomicUsize::new(0));
    Arc::new(move |img: &Img| {
        let n = count.fetch_add(1, Ordering::Relaxed);
        let name = format!("snapshot-{n:04}.png");
        if write_png(&img.to_rgb8(), &dir.join(&name)).is_ok() {
            println!("{}", serde_json::json!({ "snapshot": name }));
        }
    })
}

/// Read an 8-bit RGB image, with decoder limits.
pub fn read_png(path: &Path) -> Result<Img, String> {
    let fail = |e: &dyn std::fmt::Display| format!("cannot read {}: {e}", path.display());
    let mut reader = image::ImageReader::open(path).map_err(|e| fail(&e))?.with_guessed_format().map_err(|e| fail(&e))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(30_000);
    limits.max_image_height = Some(30_000);
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    Ok(Img::from_rgb8(&reader.decode().map_err(|e| fail(&e))?.to_rgb8()))
}

/// Write an 8-bit RGB PNG quickly (unfiltered, fast compression), which also
/// keeps decoding trivial for simple plug-ins.
pub fn write_png(img: &image::RgbImage, path: &Path) -> std::io::Result<()> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    PngEncoder::new_with_quality(file, CompressionType::Fast, FilterType::NoFilter)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Setting, Value};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("photo-airt-serve-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn desc() -> Description {
        Description {
            id: "demo".into(),
            name: "Demo".into(),
            settings: vec![Setting::number("size", "Size", 1.0, 10.0, 5.0, 1.0)],
            ..Default::default()
        }
    }

    /// Writes an input image and a request; returns the request's path and the output's.
    fn request(dir: &Path, edit: impl FnOnce(&mut serde_json::Value)) -> (PathBuf, PathBuf) {
        let (input, output, path) = (dir.join("in.png"), dir.join("out.png"), dir.join("request.json"));
        write_png(&image::RgbImage::from_pixel(6, 4, image::Rgb([10, 20, 30])), &input).unwrap();
        let mut json = serde_json::json!({
            "protocol": PROTOCOL, "plugin_id": "demo", "input": input, "output": output,
            "width": 6, "height": 4, "params": {"size": 99.0}, "seed": 42, "scale": 0.5,
        });
        edit(&mut json);
        std::fs::write(&path, json.to_string()).unwrap();
        (path, output)
    }

    fn args(path: &Path) -> Vec<String> {
        vec!["render".into(), path.display().to_string()]
    }

    #[test]
    fn a_render_gets_sanitized_settings_and_writes_its_result() {
        let dir = tmp("ok");
        let (path, output) = request(&dir, |_| {});
        render_command(&args(&path), &desc(), |img, params, ctx| {
            assert_eq!((img.w, img.h), (6, 4));
            assert_eq!(params.0["size"], Value::Number(10.0), "clamped to the setting's range");
            assert_eq!(ctx.seed, 42);
            assert!(!ctx.recording());
            Ok(img.map(|_| [1.0, 0.0, 0.0]))
        })
        .unwrap();
        let out = read_png(&output).unwrap();
        assert_eq!((out.w, out.h, out.at(0, 0)), (6, 4, [1.0, 0.0, 0.0]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshots_go_to_the_requested_folder_in_order() {
        let dir = tmp("snapshots");
        let frames = dir.join("frames");
        std::fs::create_dir_all(&frames).unwrap();
        let (path, _) = request(&dir, |r| r["snapshots"] = serde_json::json!(frames));
        render_command(&args(&path), &desc(), |img, _, ctx| {
            assert!(ctx.recording());
            ctx.snapshot(img);
            ctx.snapshot(img);
            Ok(img.clone())
        })
        .unwrap();
        assert!(frames.join("snapshot-0000.png").is_file() && frames.join("snapshot-0001.png").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bad_requests_are_explained_and_never_render() {
        let dir = tmp("bad");
        let never = |_: &Img, _: &Params, _: &Ctx| -> Result<Img, String> { panic!("must not render") };
        assert_eq!(render_command(&[], &desc(), never).unwrap_err(), USAGE);
        assert_eq!(render_command(&["paint".into(), "x".into()], &desc(), never).unwrap_err(), USAGE);

        let missing = render_command(&args(&dir.join("nope.json")), &desc(), never).unwrap_err();
        assert!(missing.contains("cannot read"), "{missing}");

        let garbage = dir.join("garbage.json");
        std::fs::write(&garbage, "{not json").unwrap();
        assert!(render_command(&args(&garbage), &desc(), never).unwrap_err().contains("invalid request"));

        let (path, _) = request(&dir, |r| r["protocol"] = serde_json::json!(999));
        assert_eq!(render_command(&args(&path), &desc(), never).unwrap_err(), "unsupported contract version 999");

        let (path, _) = request(&dir, |r| r["input"] = serde_json::json!(dir.join("no-such.png")));
        assert!(render_command(&args(&path), &desc(), never).unwrap_err().contains("cannot read"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_render_reports_its_error_and_writes_nothing() {
        let dir = tmp("failed");
        let (path, output) = request(&dir, |_| {});
        let err = render_command(&args(&path), &desc(), |_, _, _| Err("out of ink".to_string())).unwrap_err();
        assert_eq!(err, "out of ink");
        assert!(!output.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unwritable_output_is_an_error() {
        let dir = tmp("unwritable");
        let (path, _) = request(&dir, |r| r["output"] = serde_json::json!(dir.join("no-folder").join("out.png")));
        let err = render_command(&args(&path), &desc(), |img, _, _| Ok(img.clone())).unwrap_err();
        assert!(err.contains("cannot write"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn png_files_round_trip_and_junk_is_refused() {
        let dir = tmp("png");
        let path = dir.join("a.png");
        let img = image::RgbImage::from_fn(3, 2, |x, y| image::Rgb([x as u8 * 40, y as u8 * 80, 7]));
        write_png(&img, &path).unwrap();
        assert_eq!(read_png(&path).unwrap().to_rgb8(), img);
        std::fs::write(&path, b"not a png").unwrap();
        assert!(read_png(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
