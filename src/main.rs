use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use glide::engine::scene::Scene;
use glide::export::{self, Job, Source};
use glide::project::model::{Codec, ExportFormat, ExportSettings, Quality};
use glide::project::{self, Package};
use glide::render::{Gpu, Renderer, VideoFrame};

#[derive(Parser)]
#[command(name = "glide", version, about = "Fast native screen recorder and editor")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Render one frame of a project to PNG.
    Frame {
        /// A .glide package (omit with --synthetic).
        project: Option<PathBuf>,
        /// Use the built-in synthetic recording.
        #[arg(long)]
        synthetic: bool,
        /// Playback time in seconds.
        #[arg(short, long, default_value_t = 2.0)]
        time: f64,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        /// Apply a style preset JSON (partial project.json) before rendering.
        #[arg(long)]
        style: Option<String>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Export a project to MP4 or GIF.
    Export {
        project: Option<PathBuf>,
        #[arg(long)]
        synthetic: bool,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, value_parser = ["mp4", "gif"])]
        format: Option<String>,
        #[arg(long)]
        height: Option<u32>,
        #[arg(long)]
        fps: Option<u32>,
        #[arg(long, value_parser = ["studio", "social", "web", "web-low"])]
        quality: Option<String>,
        #[arg(long, value_parser = ["h264", "hevc"])]
        codec: Option<String>,
    },
    /// Record the screen into a new .glide project.
    Record {
        /// Display id (see `glide devices`); default: main display.
        #[arg(long)]
        display: Option<u32>,
        /// Window id (see `glide devices`).
        #[arg(long)]
        window: Option<u32>,
        /// Area as x,y,w,h in points on the display.
        #[arg(long)]
        area: Option<String>,
        /// Record a generated test pattern instead of the screen.
        #[arg(long)]
        pattern: bool,
        #[arg(long)]
        mic: bool,
        #[arg(long)]
        no_system_audio: bool,
        #[arg(long)]
        camera: bool,
        /// Stop after this many seconds (else Ctrl-C).
        #[arg(short = 't', long)]
        duration: Option<f64>,
        #[arg(long, default_value_t = 60)]
        fps: u32,
        /// Folder for the project (default ~/Movies/Glide).
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Open the app (the default with no subcommand).
    Gui {
        /// A .glide project to open.
        project: Option<PathBuf>,
    },
    /// Render the UI headlessly to PNG (layout checks without a display).
    #[command(hide = true)]
    Snapshot {
        project: Option<PathBuf>,
        #[arg(long)]
        tab: Option<String>,
        /// Select the first zoom, mask or slice.
        #[arg(long, value_parser = ["zoom", "mask", "slice"])]
        select: Option<String>,
        #[arg(long)]
        export_sheet: bool,
        #[arg(long)]
        light: bool,
        #[arg(long, default_value_t = 0.0)]
        at: f64,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Write the app icon as a macOS .iconset folder (or one PNG with --png).
    #[command(hide = true)]
    Icon {
        output: PathBuf,
        #[arg(long)]
        png: Option<u32>,
    },
    /// Save the current desktop picture as PNG (what "Desktop wallpaper" uses).
    #[command(hide = true)]
    Wallpaper { output: PathBuf },
    /// Transcribe a project's microphone track into captions.
    Transcribe { project: PathBuf },
    /// List displays, windows, cameras and microphones.
    Devices,
    /// Show (or request) Screen Recording, Microphone and Camera access.
    Permissions {
        #[arg(long)]
        request: bool,
    },
    /// Write a synthetic recording package (no permissions needed).
    Synth {
        output: PathBuf,
        #[arg(long, default_value_t = 12.0)]
        duration: f64,
        #[arg(long, default_value_t = 2560)]
        width: u32,
        #[arg(long, default_value_t = 1600)]
        height: u32,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // AppKit must be initialised for cursor and device queries.
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        let _ = objc2_app_kit::NSApplication::sharedApplication(mtm);
    }
    match cli.cmd {
        Some(Cmd::Frame { project, synthetic, time, height, style, output }) => {
            frame(project, synthetic, time, height, style, output)
        }
        Some(Cmd::Export { project, synthetic, output, format, height, fps, quality, codec }) => {
            export_cmd(project, synthetic, output, format, height, fps, quality, codec)
        }
        Some(Cmd::Synth { output, duration, width, height }) => synth(output, duration, width, height),
        Some(Cmd::Record { display, window, area, pattern, mic, no_system_audio, camera, duration, fps, dir, name }) => {
            record(display, window, area, pattern, mic, !no_system_audio, camera, duration, fps, dir, name)
        }
        Some(Cmd::Devices) => devices(),
        Some(Cmd::Permissions { request }) => permissions(request),
        Some(Cmd::Gui { project }) => glide::app::run(project),
        Some(Cmd::Wallpaper { output }) => {
            glide::app::macos::save_desktop_wallpaper(&output)?;
            println!("{}", output.display());
            Ok(())
        }
        Some(Cmd::Icon { output, png }) => {
            match png {
                Some(size) => image::save_buffer(&output, &glide::render::icon::render(size), size, size, image::ExtendedColorType::Rgba8)?,
                None => glide::render::icon::write_iconset(&output)?,
            }
            println!("{}", output.display());
            Ok(())
        }
        Some(Cmd::Snapshot { project, tab, select, export_sheet, light, at, output }) => {
            use glide::app::editor::Tab;
            let tab = tab.map(|t| {
                Tab::ALL.iter().find(|(_, n)| n.eq_ignore_ascii_case(&t)).map(|(t, _)| *t).expect("unknown tab")
            });
            let shot = glide::app::snapshot::Shot {
                project: project.as_deref(),
                tab,
                select,
                export_sheet,
                dark: !light,
                playhead_ms: at * 1000.0,
                size: (1320.0, 840.0),
            };
            glide::app::snapshot::render(&shot, &output)?;
            println!("{}", output.display());
            Ok(())
        }
        Some(Cmd::Transcribe { project }) => {
            let n = glide::transcribe::transcribe_package(&Package::new(&project))?;
            println!("{n} words → {}", glide::transcribe::transcript_path(&Package::new(&project)).display());
            Ok(())
        }
        None => glide::app::run(None),
    }
}

fn frame(pkg: Option<PathBuf>, synthetic: bool, time: f64, height: u32, style: Option<String>, output: PathBuf) -> Result<()> {
    let (mut proj, rec, dir) = if synthetic {
        let rec = glide::synth::recording(1920, 1080, 12_000.0);
        (project::new_project("synthetic", &rec), rec, None)
    } else {
        let pkg = Package::new(pkg.expect("project path or --synthetic"));
        let (p, r) = pkg.load()?;
        (p, r, Some(pkg.root.clone()))
    };
    if let Some(s) = style {
        let mut v = serde_json::to_value(&proj)?;
        merge(&mut v, serde_json::from_str(&s)?);
        proj = serde_json::from_value(v)?;
    }
    let mut scene = Scene::new(proj.clone(), &rec, 60.0);
    let f = scene.frame(time * 1000.0, height as f64);
    let gpu = Gpu::headless()?;
    let mut r = Renderer::new(gpu);
    r.prepare(&proj.style, &rec, dir.as_deref());
    if synthetic {
        let (w, h) = (rec.meta.screen.width, rec.meta.screen.height);
        let px = glide::synth::screen_bgra(w, h, f.source_ms);
        r.set_screen(&VideoFrame { width: w, height: h, stride: (w * 4) as usize, bgra: &px });
    } else if let Some(dir) = &dir {
        let pkg = Package::new(dir);
        let mut src = glide::media::FrameSource::open(&pkg.rec(&rec.meta.screen.file))?;
        if let Some(vf) = src.frame_at(f.source_ms) {
            vf.with_bgra(|v| r.set_screen(v));
        }
        if let Some(cam) = &rec.meta.camera
            && let Ok(mut c) = glide::media::FrameSource::open(&pkg.rec(&cam.file))
            && let Some(vf) = c.frame_at(f.source_ms - cam.offset_ms)
        {
            vf.with_bgra(|v| r.set_camera(v));
        }
    }
    let img = r.render_to_image(&f, &proj.style);
    image::save_buffer(&output, &img.rgba, img.width, img.height, image::ExtendedColorType::Rgba8)?;
    println!("{} ({}x{}, source {:.0} ms)", output.display(), img.width, img.height, f.source_ms);
    Ok(())
}

/// Deep-merge JSON objects (b into a).
fn merge(a: &mut serde_json::Value, b: serde_json::Value) {
    match (a, b) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                merge(a.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (a, b) => *a = b,
    }
}

#[allow(clippy::too_many_arguments)]
fn export_cmd(
    pkg: Option<PathBuf>,
    synthetic: bool,
    output: Option<PathBuf>,
    format: Option<String>,
    height: Option<u32>,
    fps: Option<u32>,
    quality: Option<String>,
    codec: Option<String>,
) -> Result<()> {
    let (proj, rec, source, base) = if synthetic {
        let rec = glide::synth::recording(1920, 1080, 12_000.0);
        (project::new_project("synthetic", &rec), rec, Source::Synthetic, PathBuf::from("synthetic.glide"))
    } else {
        let path = pkg.expect("project path or --synthetic");
        let pkg = Package::new(&path);
        let (p, r) = pkg.load()?;
        (p, r, Source::Package(path.clone()), path)
    };
    let mut settings = proj.export;
    if let Some(f) = format {
        settings = if f == "gif" { ExportSettings::gif_default() } else { ExportSettings { format: ExportFormat::Mp4, ..settings } };
    }
    if let Some(h) = height {
        settings.height = h;
    }
    if let Some(f) = fps {
        settings.fps = f;
    }
    if let Some(q) = quality {
        settings.quality = match q.as_str() {
            "studio" => Quality::Studio,
            "web" => Quality::Web,
            "web-low" => Quality::WebLow,
            _ => Quality::Social,
        };
    }
    if let Some(c) = codec {
        settings.codec = if c == "hevc" { Codec::Hevc } else { Codec::H264 };
    }
    let output = output.unwrap_or_else(|| export::default_output(&base, &settings));
    let job = Job { project: proj, recording: rec, source, settings, output: output.clone() };
    let cancel = AtomicBool::new(false);
    let mut last = Instant::now();
    let stats = export::run(&job, &mut |p| {
        if last.elapsed().as_millis() > 500 {
            eprint!("\r{:5.1}%", p * 100.0);
            last = Instant::now();
        }
    }, &cancel)?;
    eprintln!("\r100.0%");
    println!(
        "{}: {}x{} {} frames, {:.1} s video in {:.2} s ({:.1}x real time), {:.1} MB",
        output.display(),
        stats.width,
        stats.height,
        stats.frames,
        stats.video_seconds,
        stats.seconds,
        stats.speed(),
        stats.bytes as f64 / 1e6
    );
    Ok(())
}

fn synth(output: PathBuf, duration: f64, width: u32, height: u32) -> Result<()> {
    let (frames, zooms) = glide::synth::write_package(&output, duration, width, height)?;
    println!("{} ({frames} frames, {zooms} zooms)", output.display());
    Ok(())
}

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn on_sigint(_: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[allow(clippy::too_many_arguments)]
fn record(
    display: Option<u32>,
    window: Option<u32>,
    area: Option<String>,
    pattern: bool,
    mic: bool,
    system_audio: bool,
    camera: bool,
    duration: Option<f64>,
    fps: u32,
    dir: Option<PathBuf>,
    name: Option<String>,
) -> Result<()> {
    use glide::recorder::{Options, Recorder, Target};
    let target = if pattern {
        Target::Pattern
    } else if let Some(id) = window {
        Target::Window { id }
    } else if let Some(a) = area {
        let v: Vec<f64> = a.split(',').map(|x| x.trim().parse().unwrap_or(0.0)).collect();
        anyhow::ensure!(v.len() == 4, "--area expects x,y,w,h");
        Target::Area { display: display.unwrap_or(0), rect: glide::engine::geom::Rect::new(v[0], v[1], v[2], v[3]) }
    } else {
        Target::Display { id: display.unwrap_or(0) }
    };
    let opts = Options {
        target,
        system_audio: system_audio && !pattern,
        mic: mic.then_some(None),
        camera: camera.then_some(None),
        fps,
        dir: dir.unwrap_or_else(project::projects_dir),
        name,
        ..Default::default()
    };
    unsafe { libc::signal(libc::SIGINT, on_sigint as *const () as usize) };
    let rec = Recorder::start(opts)?;
    eprintln!("glide: recording to {} (Ctrl-C to stop)", rec.package.root.display());
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if STOP.load(std::sync::atomic::Ordering::SeqCst) || rec.error().is_some() {
            break;
        }
        if duration.is_some_and(|d| rec.has_frames() && rec.elapsed() >= d) {
            break;
        }
        if duration.is_some_and(|d| !rec.has_frames() && rec.started.elapsed().as_secs_f64() > d + 5.0) {
            break;
        }
    }
    if let Some(e) = rec.error() {
        eprintln!("glide: {e}");
    }
    let path = rec.stop()?;
    let pkg = Package::new(&path);
    let r = pkg.load_recording()?;
    let p = pkg.load_project()?;
    println!(
        "{} ({:.1} s, {}x{}, {} mouse events, {} keys, {} cursors, {} zooms)",
        path.display(),
        r.meta.duration_ms / 1000.0,
        r.meta.screen.width,
        r.meta.screen.height,
        r.mouse.len(),
        r.keys.len(),
        r.cursors.len(),
        p.timeline.zooms.len()
    );
    Ok(())
}

fn devices() -> Result<()> {
    use glide::capture::{camera, screen};
    println!("Displays:");
    for d in screen::displays()? {
        println!("  {:>10}  {}x{} pt  {}x{} px", d.id, d.points.0, d.points.1, d.pixels.0, d.pixels.1);
    }
    println!("Windows:");
    for w in screen::windows()? {
        println!("  {:>10}  {} — {} ({:.0}x{:.0})", w.id, w.app, w.title, w.size.0, w.size.1);
    }
    println!("Cameras:");
    for c in camera::cameras() {
        println!("  {}  ({})", c.name, c.id);
    }
    println!("Microphones:");
    for m in camera::microphones() {
        println!("  {}  ({})", m.name, m.id);
    }
    Ok(())
}

fn permissions(request: bool) -> Result<()> {
    use glide::capture::tcc;
    if request {
        tcc::request_all(true, true, true, std::time::Duration::from_secs(30));
    }
    println!("Screen Recording: {:?}", tcc::screen_status());
    println!("Microphone:       {:?}", tcc::mic_status());
    println!("Camera:           {:?}", tcc::camera_status());
    Ok(())
}
