use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use glide::engine::scene::Scene;
use glide::export::{self, Job, Source};
use glide::media::writer::{MovieWriter, WriterSpec, fill_bgra};
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
    match cli.cmd {
        Some(Cmd::Frame { project, synthetic, time, height, style, output }) => {
            frame(project, synthetic, time, height, style, output)
        }
        Some(Cmd::Export { project, synthetic, output, format, height, fps, quality, codec }) => {
            export_cmd(project, synthetic, output, format, height, fps, quality, codec)
        }
        Some(Cmd::Synth { output, duration, width, height }) => synth(output, duration, width, height),
        None => {
            println!("glide {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
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
    let ms = duration * 1000.0;
    let rec = glide::synth::recording(width, height, ms);
    let pkg = Package::new(&output);
    std::fs::create_dir_all(pkg.recording_dir())?;
    let fps = 60u32;
    let mut w = MovieWriter::create(
        &pkg.rec("screen.mov"),
        &WriterSpec { width, height, fps, codec: Codec::Hevc, bitrate: (width * height) as u64 * 8, audio: false },
    )?;
    let frames = (duration * fps as f64) as u64;
    for i in 0..frames {
        let px = glide::synth::screen_bgra(width, height, i as f64 * 1000.0 / fps as f64);
        let pb = w.buffer()?;
        fill_bgra(&pb, &px, width, height, (width * 4) as usize);
        w.push_frame(&pb)?;
    }
    w.finish(None)?;
    project::write_json(&pkg.rec("meta.json"), &rec.meta)?;
    project::write_json(&pkg.rec("mouse.json"), &rec.mouse)?;
    project::write_json(&pkg.rec("keys.json"), &rec.keys)?;
    project::write_json(&pkg.rec("cursors.json"), &rec.cursors)?;
    project::write_json(&pkg.rec("transcript.json"), &rec.transcript)?;
    let proj = project::new_project(&pkg.name(), &rec);
    pkg.save_project(&proj)?;
    println!("{} ({} frames, {} zooms)", output.display(), frames, proj.timeline.zooms.len());
    Ok(())
}
