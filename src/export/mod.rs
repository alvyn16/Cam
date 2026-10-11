//! Export: render every frame on the GPU and encode it with VideoToolbox
//! (MP4) or quantise it (GIF).
//!
//! Frames are pipelined: while frame N is read back and handed to the
//! encoder, frames N+1..N+3 are already rendering.

pub mod audio;
pub mod gif;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Result, bail};

use crate::capture::mixer::RATE;
use crate::engine::scene::Scene;
use crate::media::FrameSource;
use crate::media::writer::{MovieWriter, WriterSpec, fill_bgra};
use crate::project::model::{ExportFormat, ExportSettings, Project};
use crate::project::recording::Recording;
use crate::render::{Gpu, OUTPUT_FORMAT, Renderer, VideoFrame, gpu::padded_row};

/// Frames in flight between render and encode.
const IN_FLIGHT: usize = 3;

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
    pub video_seconds: f64,
    pub bytes: u64,
}

impl Stats {
    /// How many times faster than real time the export ran.
    pub fn speed(&self) -> f64 {
        if self.seconds > 0.0 { self.video_seconds / self.seconds } else { 0.0 }
    }
}

/// Where frames come from.
pub enum Source {
    /// A `.glide` package directory.
    Package(PathBuf),
    /// The built-in synthetic screen (tests, demos).
    Synthetic,
}

pub struct Job {
    pub project: Project,
    pub recording: Recording,
    pub source: Source,
    pub settings: ExportSettings,
    pub output: PathBuf,
}

/// Progress callback: fraction 0..1. Set `cancel` to stop.
pub type ProgressFn<'a> = &'a mut dyn FnMut(f64);

struct Inputs {
    screen: Option<FrameSource>,
    camera: Option<FrameSource>,
    camera_offset: f64,
    synthetic: Option<(u32, u32)>,
}

impl Inputs {
    fn open(job: &Job) -> Result<Self> {
        match &job.source {
            Source::Synthetic => {
                let m = &job.recording.meta.screen;
                Ok(Inputs { screen: None, camera: None, camera_offset: 0.0, synthetic: Some((m.width, m.height)) })
            }
            Source::Package(dir) => {
                let rd = dir.join("recording");
                let screen = Some(FrameSource::open(&rd.join(&job.recording.meta.screen.file))?);
                let camera = match &job.recording.meta.camera {
                    Some(c) => FrameSource::open(&rd.join(&c.file)).ok(),
                    None => None,
                };
                let camera_offset = job.recording.meta.camera.as_ref().map_or(0.0, |c| c.offset_ms);
                Ok(Inputs { screen, camera, camera_offset, synthetic: None })
            }
        }
    }

    /// Upload the media for a source time into the renderer.
    fn feed(&mut self, r: &mut Renderer, source_ms: f64) {
        if let Some((w, h)) = self.synthetic {
            let px = crate::synth::screen_bgra(w, h, source_ms);
            r.set_screen(&VideoFrame { width: w, height: h, stride: (w * 4) as usize, bgra: &px });
        }
        if let Some(s) = &mut self.screen
            && let Some(f) = s.frame_at(source_ms)
        {
            f.with_bgra(|v| r.set_screen(v));
        }
        if let Some(c) = &mut self.camera
            && let Some(f) = c.frame_at(source_ms - self.camera_offset)
        {
            f.with_bgra(|v| r.set_camera(v));
        }
    }
}

struct Readback {
    buffer: wgpu::Buffer,
    ready: Arc<AtomicBool>,
    submission: Option<wgpu::SubmissionIndex>,
}

/// Renders frames into a ring of readback buffers.
struct FramePump {
    renderer: Renderer,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    ring: Vec<Readback>,
    pending: VecDeque<usize>,
    next: usize,
    w: u32,
    h: u32,
}

impl FramePump {
    fn new(gpu: Gpu, w: u32, h: u32) -> Self {
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("export"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let ring = (0..IN_FLIGHT)
            .map(|_| Readback {
                buffer: gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("export readback"),
                    size: (padded_row(w) * h) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                ready: Arc::new(AtomicBool::new(false)),
                submission: None,
            })
            .collect();
        FramePump { renderer: Renderer::new(gpu), target, view, ring, pending: VecDeque::new(), next: 0, w, h }
    }

    fn submit(&mut self, frame: &crate::engine::scene::Frame, style: &crate::project::model::Style) {
        let gpu = self.renderer.gpu().clone();
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.renderer.render(frame, style, &self.view, &mut enc);
        let slot = self.next;
        self.next = (self.next + 1) % self.ring.len();
        let rb = &mut self.ring[slot];
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &rb.buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row(self.w)), rows_per_image: Some(self.h) },
            },
            wgpu::Extent3d { width: self.w, height: self.h, depth_or_array_layers: 1 },
        );
        rb.submission = Some(gpu.queue.submit([enc.finish()]));
        rb.ready.store(false, Ordering::SeqCst);
        let flag = rb.ready.clone();
        rb.buffer.slice(..).map_async(wgpu::MapMode::Read, move |_| flag.store(true, Ordering::SeqCst));
        self.pending.push_back(slot);
    }

    fn full(&self) -> bool {
        self.pending.len() >= self.ring.len()
    }

    /// Wait for the oldest frame and hand its rows (BGRA, padded stride) to `f`.
    fn take<R>(&mut self, f: impl FnOnce(&[u8], usize) -> R) -> Option<R> {
        let slot = self.pending.pop_front()?;
        let gpu = self.renderer.gpu().clone();
        let rb = &self.ring[slot];
        while !rb.ready.load(Ordering::SeqCst) {
            let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: rb.submission.clone(), timeout: None });
        }
        let r = {
            let data = rb.buffer.slice(..).get_mapped_range().expect("mapped frame");
            f(&data, padded_row(self.w) as usize)
        };
        rb.buffer.unmap();
        Some(r)
    }
}

/// Run an export job. `progress` receives 0..1; `cancel` aborts.
pub fn run(job: &Job, progress: ProgressFn, cancel: &AtomicBool) -> Result<Stats> {
    let started = Instant::now();
    let s = job.settings;
    let mut scene = Scene::new(job.project.clone(), &job.recording, s.fps as f64);
    let (w, h) = scene.sizes.render_size(s.height);
    let total = scene.frame_count().max(1);
    let gpu = Gpu::headless()?;
    let mut pump = FramePump::new(gpu, w, h);
    let dir = match &job.source {
        Source::Package(d) => Some(d.as_path()),
        Source::Synthetic => None,
    };
    pump.renderer.prepare(&job.project.style, &job.recording, dir);
    let mut inputs = Inputs::open(job)?;
    if let Some(parent) = job.output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let result = match s.format {
        ExportFormat::Mp4 => {
            let soundtrack = audio::mix(&job.project, &job.recording, dir, &scene.map);
            let mut writer = MovieWriter::create(
                &job.output,
                &WriterSpec { width: w, height: h, fps: s.fps, codec: s.codec, bitrate: s.bitrate(w, h), audio: soundtrack.is_some() },
            )?;
            let mut done = 0u64;
            let encode = |pump: &mut FramePump, writer: &mut MovieWriter, done: &mut u64| -> Result<()> {
                let pb = writer.buffer()?;
                pump.take(|rows, stride| fill_bgra(&pb, rows, w, h, stride));
                writer.push_frame(&pb)?;
                *done += 1;
                if let Some(m) = &soundtrack {
                    let until = ((*done as f64 / s.fps as f64 + 0.5) * RATE) as usize;
                    writer.push_audio_until(m, until)?;
                }
                Ok(())
            };
            for i in 0..total {
                if cancel.load(Ordering::SeqCst) {
                    writer.cancel();
                    let _ = std::fs::remove_file(&job.output);
                    bail!("export cancelled");
                }
                let t = i as f64 * 1000.0 / s.fps as f64;
                let frame = scene.frame(t, h as f64);
                inputs.feed(&mut pump.renderer, frame.source_ms);
                pump.submit(&frame, &job.project.style);
                if pump.full() {
                    encode(&mut pump, &mut writer, &mut done)?;
                }
                progress(i as f64 / total as f64);
            }
            while !pump.pending.is_empty() {
                encode(&mut pump, &mut writer, &mut done)?;
            }
            writer.finish(soundtrack.as_deref())?;
            Ok(done)
        }
        ExportFormat::Gif => gif::encode(job, &mut scene, &mut pump, &mut inputs, total, progress, cancel),
    };
    let frames = result?;
    progress(1.0);
    let bytes = std::fs::metadata(&job.output).map(|m| m.len()).unwrap_or(0);
    Ok(Stats {
        frames,
        width: w,
        height: h,
        seconds: started.elapsed().as_secs_f64(),
        video_seconds: scene.duration() / 1000.0,
        bytes,
    })
}

/// Default output path next to the package.
pub fn default_output(package: &Path, settings: &ExportSettings) -> PathBuf {
    let ext = if settings.format == ExportFormat::Gif { "gif" } else { "mp4" };
    let name = package.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into());
    package.with_file_name(format!("{name}.{ext}"))
}
