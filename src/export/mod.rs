//! Export: render every frame on the GPU and encode it with VideoToolbox
//! (MP4) or quantise it (GIF).
//!
//! Frames are pipelined: while frame N is read back and handed to the
//! encoder, frames N+1..N+3 are already rendering.

pub mod audio;
pub mod isolate;
pub mod gif;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Result, bail};

use crate::engine::scene::Scene;
use crate::media::{FrameSource, Prefetch};
use crate::media::writer::{MovieWriter, WriterSpec, fill_bgra};
use crate::project::model::{ExportFormat, ExportSettings, Project};
use crate::project::recording::Recording;
use crate::render::interop::{TextureCache, Wrapped};
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
#[derive(Clone)]
pub enum Source {
    /// A `.cam` package directory.
    Package(PathBuf),
    /// The built-in synthetic screen (tests, demos).
    Synthetic,
}

#[derive(Clone)]
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
    /// Background decoders keyed by output frame index.
    pre_screen: Option<Prefetch>,
    pre_camera: Option<Prefetch>,
}

impl Inputs {
    fn open(job: &Job) -> Result<Self> {
        match &job.source {
            Source::Synthetic => {
                let m = &job.recording.meta.screen;
                Ok(Inputs {
                    screen: None,
                    camera: None,
                    camera_offset: 0.0,
                    synthetic: Some((m.width, m.height)),
                    pre_screen: None,
                    pre_camera: None,
                })
            }
            Source::Package(dir) => {
                let rd = dir.join("recording");
                let screen = Some(FrameSource::open(&rd.join(&job.recording.meta.screen.file))?);
                let camera = match &job.recording.meta.camera {
                    Some(c) => FrameSource::open(&rd.join(&c.file)).ok(),
                    None => None,
                };
                let camera_offset = job.recording.meta.camera.as_ref().map_or(0.0, |c| c.offset_ms);
                Ok(Inputs { screen, camera, camera_offset, synthetic: None, pre_screen: None, pre_camera: None })
            }
        }
    }

    /// Start background decoding for the source time of every output frame.
    fn prefetch(&mut self, job: &Job, times: &[f64]) {
        let Source::Package(dir) = &job.source else { return };
        let rd = dir.join("recording");
        if self.screen.is_some() {
            self.pre_screen = Prefetch::spawn(rd.join(&job.recording.meta.screen.file), times.to_vec()).ok();
        }
        if let (Some(_), Some(c)) = (&self.camera, &job.recording.meta.camera) {
            let shifted: Vec<f64> = times.iter().map(|t| t - self.camera_offset).collect();
            self.pre_camera = Prefetch::spawn(rd.join(&c.file), shifted).ok();
        }
    }

    /// Upload the media for output frame `i` (prefetched) or a source time.
    fn feed_index(&mut self, r: &mut Renderer, i: usize, source_ms: f64, cache: Option<&TextureCache>) {
        if self.pre_screen.is_none() && self.pre_camera.is_none() {
            return self.feed(r, source_ms, cache);
        }
        if let Some(f) = self.pre_screen.as_ref().and_then(|p| p.get(i)) {
            match f.try_wrap(cache) {
                Some(w) => r.set_screen_wrapped(w),
                None => f.with_bgra(|v| r.set_screen(v)),
            }
        }
        if let Some(f) = self.pre_camera.as_ref().and_then(|p| p.get(i)) {
            match f.try_wrap(cache) {
                Some(w) => r.set_camera_wrapped(w),
                None => f.with_bgra(|v| r.set_camera(v)),
            }
        }
    }

    /// Upload the media for a source time into the renderer. With a
    /// texture cache, decoded frames are used in place (zero copy).
    fn feed(&mut self, r: &mut Renderer, source_ms: f64, cache: Option<&TextureCache>) {
        if let Some((w, h)) = self.synthetic {
            let px = crate::synth::screen_bgra(w, h, source_ms);
            r.set_screen(&VideoFrame { width: w, height: h, stride: (w * 4) as usize, bgra: &px });
        }
        if let Some(s) = &mut self.screen
            && let Some(f) = s.frame_at(source_ms)
        {
            match f.try_wrap(cache) {
                Some(w) => r.set_screen_wrapped(w),
                None => f.with_bgra(|v| r.set_screen(v)),
            }
        }
        if let Some(c) = &mut self.camera
            && let Some(f) = c.frame_at(source_ms - self.camera_offset)
        {
            match f.try_wrap(cache) {
                Some(w) => r.set_camera_wrapped(w),
                None => f.with_bgra(|v| r.set_camera(v)),
            }
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
            label: Some("export target"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut ring = Vec::with_capacity(IN_FLIGHT);
        for _ in 0..IN_FLIGHT {
            ring.push(Readback {
                buffer: gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: (padded_row(w) * h) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                ready: Arc::new(AtomicBool::new(false)),
                submission: None,
            });
        }
        FramePump {
            renderer: Renderer::new(gpu),
            target,
            view,
            ring,
            pending: VecDeque::new(),
            next: 0,
            w,
            h,
        }
    }

    fn submit(&mut self, frame: &crate::engine::scene::Frame, style: &crate::project::model::Style) {
        let idx = self.next;
        self.next = (self.next + 1) % self.ring.len();
        let mut enc = self.renderer.gpu().device.create_command_encoder(&Default::default());
        self.renderer.render(frame, style, &self.view, &mut enc);
        let rb = &mut self.ring[idx];
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &rb.buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row(self.w)), rows_per_image: Some(self.h) },
            },
            wgpu::Extent3d { width: self.w, height: self.h, depth_or_array_layers: 1 },
        );
        rb.ready.store(false, Ordering::SeqCst);
        let sub = self.renderer.gpu().queue.submit([enc.finish()]);
        rb.submission = Some(sub);
        let flag = rb.ready.clone();
        rb.buffer.slice(..).map_async(wgpu::MapMode::Read, move |_| flag.store(true, Ordering::SeqCst));
        self.pending.push_back(idx);
    }

    fn full(&self) -> bool {
        self.pending.len() >= IN_FLIGHT
    }

    /// Read the oldest submitted frame. Blocks until the GPU is done with it.
    fn take<R>(&mut self, f: impl FnOnce(&[u8], usize) -> R) -> Option<R> {
        let idx = self.pending.pop_front()?;
        let rb = &self.ring[idx];
        let gpu = self.renderer.gpu();
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
    export(job.clone(), progress, cancel)
}

/// Run an export job to completion. `progress` is called with the
/// fraction done (0..1). Set `cancel` to stop early; unfinished output is
/// deleted.
pub fn export(job: Job, progress: ProgressFn, cancel: &AtomicBool) -> Result<Stats> {
    let started = Instant::now();
    let s = job.settings;
    let mut scene = Scene::new(job.project.clone(), &job.recording, s.fps as f64);
    let (w, h) = scene.sizes.render_size(s.height);
    let total = scene.frame_count().max(1);
    if total == 0 {
        bail!("empty export");
    }
    let gpu = Gpu::headless()?;
    let mut pump = FramePump::new(gpu, w, h);
    let dir = match &job.source {
        Source::Package(d) => Some(d.as_path()),
        Source::Synthetic => None,
    };
    pump.renderer.prepare(&job.project.style, &job.recording, dir);
    let mut inputs = Inputs::open(&job)?;
    // Build the list of source timestamps so prefetch can decode ahead.
    let times: Vec<f64> = (0..total)
        .map(|i| scene.map.to_source(i as f64 * 1000.0 / s.fps as f64))
        .collect();
    inputs.prefetch(&job, &times);
    let result = match s.format {
        ExportFormat::Mp4 => {
            let bitrate = s.bitrate(w, h, scene.duration() / 1000.0);
            let soundtrack = audio::mix(&job.project, &job.recording, dir, &scene.map);
            let mut writer = MovieWriter::create(
                &job.output,
                &WriterSpec {
                    width: w,
                    height: h,
                    fps: s.fps,
                    codec: s.codec_for(w, h),
                    bitrate,
                    quality: (s.target_mb.is_none() || !s.fits_size()).then(|| s.quality.encoder_quality()),
                    audio: soundtrack.is_some(),
                },
            )?;
            if let Some(m) = soundtrack {
                writer.set_soundtrack(Arc::new(m));
            }
            let cache = TextureCache::new(&pump.renderer.gpu().device);
            let result = match &cache {
                Some(cache) => mp4_zero_copy(&job, &mut scene, &mut pump, &mut inputs, cache, &mut writer, total, progress, cancel),
                None => mp4_readback(&job, &mut scene, &mut pump, &mut inputs, &mut writer, total, progress, cancel),
            };
            match result {
                Ok(done) => {
                    writer.finish()?;
                    Ok(done)
                }
                Err(e) => {
                    writer.cancel();
                    let _ = std::fs::remove_file(&job.output);
                    Err(e)
                }
            }
        }
        ExportFormat::Gif => gif::encode(&job, &mut scene, &mut pump, &mut inputs, total, progress, cancel),
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

#[cfg(target_os = "macos")]
/// Render straight into the encoder's IOSurface pixel buffers.
#[allow(clippy::too_many_arguments)]
fn mp4_zero_copy(
    job: &Job,
    scene: &mut Scene,
    pump: &mut FramePump,
    inputs: &mut Inputs,
    cache: &TextureCache,
    writer: &mut MovieWriter,
    total: u64,
    progress: ProgressFn,
    cancel: &AtomicBool,
) -> Result<u64> {
    let s = job.settings;
    let h = pump.h;
    let gpu = pump.renderer.gpu().clone();
    let mut inflight: VecDeque<(Wrapped, wgpu::SubmissionIndex)> = VecDeque::new();
    let mut done = 0u64;
    let profile = std::env::var_os("CAM_PROFILE").is_some();
    let mut prof = [0.0f64; 5];
    let mut finish_oldest = |inflight: &mut VecDeque<(Wrapped, wgpu::SubmissionIndex)>, writer: &mut MovieWriter, prof: &mut [f64; 5]| -> Result<()> {
        if let Some((target, idx)) = inflight.pop_front() {
            let t = Instant::now();
            let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None });
            prof[3] += t.elapsed().as_secs_f64();
            let t = Instant::now();
            writer.push_frame(&target.pb.0)?;
            prof[4] += t.elapsed().as_secs_f64();
            done += 1;
        }
        Ok(())
    };
    for i in 0..total {
        if cancel.load(Ordering::SeqCst) {
            bail!("export cancelled");
        }
        let t = Instant::now();
        let frame = scene.frame(i as f64 * 1000.0 / s.fps as f64, h as f64);
        prof[0] += t.elapsed().as_secs_f64();
        let t = Instant::now();
        inputs.feed_index(&mut pump.renderer, i as usize, frame.source_ms, Some(cache));
        prof[1] += t.elapsed().as_secs_f64();
        let t = Instant::now();
        let pb = writer.buffer()?;
        let target = cache
            .wrap(&pb, wgpu::TextureUsages::RENDER_ATTACHMENT)
            .ok_or_else(|| anyhow::anyhow!("cannot wrap encoder buffer"))?;
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        pump.renderer.render(&frame, &job.project.style, &target.view, &mut enc);
        let idx = gpu.queue.submit([enc.finish()]);
        prof[2] += t.elapsed().as_secs_f64();
        inflight.push_back((target, idx));
        if inflight.len() >= IN_FLIGHT {
            finish_oldest(&mut inflight, writer, &mut prof)?;
        }
        if i % 120 == 0 {
            cache.flush();
        }
        progress(i as f64 / total as f64);
    }
    while !inflight.is_empty() {
        finish_oldest(&mut inflight, writer, &mut prof)?;
    }
    if profile {
        eprintln!(
            "\nprofile: scene {:.2}s  decode+upload {:.2}s  render+submit {:.2}s  gpu wait {:.2}s  encoder {:.2}s",
            prof[0], prof[1], prof[2], prof[3], prof[4]
        );
    }
    Ok(done)
}

#[allow(clippy::too_many_arguments)]
fn mp4_zero_copy(
    _job: &Job,
    _scene: &mut Scene,
    _pump: &mut FramePump,
    _inputs: &mut Inputs,
    _cache: &TextureCache,
    _writer: &mut MovieWriter,
    _total: u64,
    _progress: ProgressFn,
    _cancel: &AtomicBool,
) -> Result<u64> {
    bail!("zero-copy export is only supported on macOS");
}

/// Fallback: render, read back through the CPU, copy into pixel buffers.
#[allow(clippy::too_many_arguments)]
fn mp4_readback(
    job: &Job,
    scene: &mut Scene,
    pump: &mut FramePump,
    inputs: &mut Inputs,
    writer: &mut MovieWriter,
    total: u64,
    progress: ProgressFn,
    cancel: &AtomicBool,
) -> Result<u64> {
    let s = job.settings;
    let (w, h) = (pump.w, pump.h);
    let mut done = 0u64;
    let encode = |pump: &mut FramePump, writer: &mut MovieWriter, done: &mut u64| -> Result<()> {
        let pb = writer.buffer()?;
        if pump.take(|rows, stride| fill_bgra(&pb, rows, w, h, stride)).is_some() {
            writer.push_frame(&pb)?;
            *done += 1;
        }
        Ok(())
    };
    for i in 0..total {
        if cancel.load(Ordering::SeqCst) {
            bail!("export cancelled");
        }
        let frame = scene.frame(i as f64 * 1000.0 / s.fps as f64, h as f64);
        inputs.feed_index(&mut pump.renderer, i as usize, frame.source_ms, None);
        pump.submit(&frame, &job.project.style);
        if pump.full() {
            encode(pump, writer, &mut done)?;
        }
        progress(i as f64 / total as f64);
    }
    while !pump.pending.is_empty() {
        encode(pump, writer, &mut done)?;
    }
    Ok(done)
}

/// Default output path next to the package.
pub fn default_output(package: &Path, settings: &ExportSettings) -> PathBuf {
    let ext = match (settings.format, settings.codec) {
        (ExportFormat::Gif, _) => "gif",
        (ExportFormat::Mp4, crate::project::model::Codec::Prores) => "mov",
        (ExportFormat::Mp4, _) => "mp4",
    };
    let stem = package.file_stem().and_then(|s| s.to_str()).unwrap_or("export");
    let name = format!("{stem}.{ext}");
    package.parent().unwrap_or_else(|| Path::new(".")).join(name)
}

impl Source {
    pub fn package_dir(&self) -> Option<&Path> {
        match self {
            Source::Package(p) => Some(p.as_path()),
            Source::Synthetic => None,
        }
    }
}
