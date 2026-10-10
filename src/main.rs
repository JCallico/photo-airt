mod ai;
mod app;
mod app_open;
mod finish;
mod photo_io;
mod plugins;
mod sources;
mod theme;
mod ui_canvas;
mod ui_open;
mod ui_panels;
mod ui_plugins;

use std::path::PathBuf;

use photo_airt_sdk::imaging;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--render") {
        return headless(&args[2..]);
    }
    if args.get(1).map(String::as_str) == Some("--ai") {
        return ai_cli(&args[2..]);
    }
    match args.get(1).map(String::as_str) {
        Some("--check-plugin") => return check_plugin(&args[2..]),
        Some("--run-plugin") => return run_builtin_plugin(&args[2..]),
        Some("--plugins") => return list_plugins(),
        _ => {}
    }
    // A path or a link (https://…) to open on start.
    let initial = args.get(1).filter(|a| !a.starts_with("--")).cloned();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Photo·AIrt")
            .with_app_id("photo-airt")
            .with_inner_size([1560.0, 960.0])
            .with_min_inner_size([1100.0, 680.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Photo·AIrt", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Headless batch mode, handy for scripting and for comparing styles:
/// `photo-airt --render <style-id|all> <input> <out-dir> [long-side]`
fn headless(args: &[String]) -> anyhow::Result<()> {
    let style = args.first().map(String::as_str).unwrap_or("all");
    let input = resolve_input(args.get(1).ok_or_else(|| anyhow::anyhow!("missing input path or link"))?)?;
    let out_dir = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("."));
    let long: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2048);
    let photo = imaging::Img::from_rgb8(&photo_io::load_photo(&input)?).fit_long(long);
    std::fs::create_dir_all(&out_dir)?;
    let registry = plugins::registry();
    if style != "all" && registry.get(style).is_none() {
        anyhow::bail!("unknown style {style}");
    }
    for plugin in registry.all().iter().filter(|p| style == "all" || p.id() == style) {
        let ctx = plugins::Ctx::for_image(&photo, 7);
        let t = std::time::Instant::now();
        let out = plugin
            .render(&photo, &plugins::Params::defaults(plugin.description()), &ctx)
            .map_err(|e| anyhow::anyhow!("{}: {e}", plugin.id()))?;
        println!("{:<14} {:>6} ms", plugin.id(), t.elapsed().as_millis());
        out.to_rgb8().save(out_dir.join(format!("{}.jpg", plugin.id())))?;
    }
    Ok(())
}

/// Run one AI job from the terminal, streaming its log:
/// `photo-airt --ai <director|vector|repaint|duet|placard> <input> [out.png]`
fn ai_cli(args: &[String]) -> anyhow::Result<()> {
    use std::sync::Arc;
    let what = args.first().map(String::as_str).unwrap_or("director");
    let input = resolve_input(args.get(1).ok_or_else(|| anyhow::anyhow!("missing input path or link"))?)?;
    let photo = Arc::new(imaging::Img::from_rgb8(&photo_io::load_photo(&input)?).fit_long(1600));
    let (tx, rx) = std::sync::mpsc::channel();
    let mut hub = ai::AiHub::new(tx, egui::Context::default());
    // Roles come from the environment for testing, e.g.
    // PHOTO_AIRT_DIRECTOR=codex:gpt-5.6-luna PHOTO_AIRT_PAINTER=claude:sonnet
    let role = |var: &str, default: ai::Cli| {
        let v = std::env::var(var).unwrap_or_default();
        let (cli, model) = v.split_once(':').unwrap_or((v.as_str(), ""));
        let cli = match cli {
            "claude" => ai::Cli::Claude,
            "codex" => ai::Cli::Codex,
            _ => default,
        };
        ai::RoleCfg { cli, model: model.to_string() }
    };
    let director = role("PHOTO_AIRT_DIRECTOR", ai::Cli::Claude);
    let painter = role("PHOTO_AIRT_PAINTER", ai::Cli::Codex);
    println!("director: {}  ·  painter: {}", director.describe(), painter.describe());
    let job = match what {
        "director" => hub.art_director(&director, photo),
        "vector" => hub.vector(&director, photo, &ai::VECTOR_STYLES[0]),
        "repaint" | "duet" => {
            let brief = (what == "duet").then_some(&director);
            hub.repaint(&painter, brief, photo, "Watercolour".into(), ai::PAINT_PRESETS[1].prompt.into())
        }
        "placard" => hub.placard(&director, 1, photo, "the original photograph".into()),
        other => anyhow::bail!("unknown AI job {other}"),
    };
    let mut seen = 0;
    loop {
        let ev = rx.recv_timeout(std::time::Duration::from_millis(300));
        {
            let s = job.shared.lock().unwrap();
            for line in &s.log[seen..] {
                println!("[{:>5.1}s] {line}", job.started.elapsed().as_secs_f32());
            }
            seen = s.log.len();
        }
        let Ok(ev) = ev else { continue };
        match ev.result {
            Err(e) => anyhow::bail!("job failed: {e}"),
            Ok(ai::AiOutput::Director(r)) => println!("{r:#?}"),
            Ok(ai::AiOutput::Placard { placard, .. }) => println!("{placard:#?}"),
            Ok(ai::AiOutput::Image { title, img, saved, prompt, .. }) => {
                println!("{title}: {}x{} saved to {}\nprompt: {prompt}", img.width(), img.height(), saved.display());
                if let Some(out) = args.get(2) {
                    img.save(out)?;
                }
            }
        }
        return Ok(());
    }
}

/// Command-line inputs may be paths or links; links are downloaded with the
/// same safeguards as in the app.
fn resolve_input(arg: &str) -> anyhow::Result<PathBuf> {
    let input = sources::classify(arg).map_err(|e| anyhow::anyhow!("{arg}: {e}"))?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut last = String::new();
    let asset = sources::acquire(
        &input,
        &mut |p| {
            if p.stage != last {
                eprintln!("{}", p.stage);
                last = p.stage;
            }
        },
        &cancel,
    )?;
    Ok(asset.local)
}

/// `photo-airt --check-plugin <folder>`: validate an external plug-in against
/// the contract without approving it.
fn check_plugin(args: &[String]) -> anyhow::Result<()> {
    let dir = PathBuf::from(args.first().ok_or_else(|| anyhow::anyhow!("usage: photo-airt --check-plugin <plug-in folder>"))?);
    let dir = std::fs::canonicalize(&dir).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
    println!("Checking {}", dir.display());
    let (report, ok) = plugins::external::check(&dir);
    for (passed, line) in &report {
        println!("  {} {line}", if *passed { "✓" } else { "✗" });
    }
    if ok {
        println!("All checks passed.");
        Ok(())
    } else {
        anyhow::bail!("the plug-in does not meet the contract")
    }
}

/// `photo-airt --run-plugin <style-id> render <request.json>`: render a
/// built-in style through the external plug-in contract, exactly as its
/// standalone executable in `plugins/<id>` would.
fn run_builtin_plugin(args: &[String]) -> anyhow::Result<()> {
    let usage = "usage: photo-airt --run-plugin <style-id> render <request.json>";
    let registry = plugins::Registry::builtin();
    let id = args.first().ok_or_else(|| anyhow::anyhow!(usage))?;
    let plugin = registry.get(id).ok_or_else(|| anyhow::anyhow!("unknown built-in style {id}"))?;
    photo_airt_sdk::render_command(&args[1..], plugin.description(), |img, params, ctx| {
        plugin.render(img, params, ctx).map_err(|e| e.to_string())
    })
    .map_err(|e| anyhow::anyhow!(e))
}

/// `photo-airt --plugins`: where plug-ins are looked for and what was found.
fn list_plugins() -> anyhow::Result<()> {
    let scan = plugins::external::scan(&plugins::Registry::builtin_ids(), &plugins::external::Trust::load());
    println!("Plug-in locations (highest precedence first):");
    for l in &scan.locations {
        println!("  {} {:<22} {}", if l.exists { "●" } else { "○" }, l.kind.label(), l.path.display());
    }
    println!("Plug-ins:");
    if scan.found.is_empty() {
        println!("  (none)");
    }
    for f in &scan.found {
        println!("  {:<24} {:<14} {}", f.name, format!("{:?}", f.status).split('(').next().unwrap_or(""), f.dir.display());
        if let plugins::external::Status::Error(e) = &f.status {
            println!("      {e}");
        }
    }
    Ok(())
}
