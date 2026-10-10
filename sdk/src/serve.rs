//! The plug-in side of the external contract, `render <request.json>`, for
//! styles written in Rust.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    let ctx = Ctx::for_image(&img, req.seed);

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
