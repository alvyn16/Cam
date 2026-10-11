use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use glide::engine::scene::Scene;
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Frame { project, synthetic, time, height, style, output }) => {
            frame(project, synthetic, time, height, style, output)
        }
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
