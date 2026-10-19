//! Windows recording session orchestration.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::capture::compose::PipSpec;
use crate::capture::screen;
use crate::capture::writer::{Codec, Stats};

#[derive(Clone, Debug)]
pub struct Options {
    pub output: PathBuf,
    pub size: (usize, usize),
    pub fps: u32,
    pub codec: Codec,
    pub bitrate: u64,
    pub quality: Option<f64>,
    pub system_audio: bool,
    pub mic: Option<Option<String>>,
    pub webcam: Option<Option<String>>,
    pub pip: PipSpec,
    pub hide_desktop_icons: bool,
    pub hide_dock: bool,
    pub exclude_own_audio: bool,
    pub mic_backend: MicBackend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Default)]
pub enum MicBackend {
    #[default]
    Auto,
    Sck,
    Avcapture,
}

pub struct Shared {
    pub sink: Mutex<Option<crate::capture::writer::Sink>>,
    pub error: Mutex<Option<String>>,
    pub stop: Arc<AtomicBool>,
}

impl Shared {
    pub fn with<R>(&self, f: impl FnOnce(&mut crate::capture::writer::Sink) -> R) -> Option<R> {
        let mut g = self.sink.lock().ok()?;
        g.as_mut().map(|sink| {
            let result = f(sink);
            if let Some(error) = sink.error() {
                *self.error.lock().unwrap() = Some(error);
            }
            result
        })
    }
}

pub struct Session {
    pub output: Option<PathBuf>,
    pub started: Instant,
    pub description: String,
    clock: Arc<Mutex<super::audio::Clock>>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    frames: Arc<std::sync::atomic::AtomicU64>,
    thread: Option<std::thread::JoinHandle<Result<Stats>>>,
    stems: Vec<super::audio::Stem>,
    desktop: Option<super::desktop::DesktopGuard>,
    cancelled: Arc<AtomicBool>,
}
impl Session {
    pub fn start(target: screen::Target, o: Options) -> Result<Self> {
        Self::start_source(Some(target), o)
    }
    pub fn start_pattern(o: Options) -> Result<Self> {
        Self::start_source(None, o)
    }
    fn start_source(target: Option<screen::Target>, o: Options) -> Result<Self> {
        anyhow::ensure!(o.fps > 0 && o.fps <= 120, "frame rate must be 1–120");
        let clock = Arc::new(Mutex::new(super::audio::Clock::default()));
        let mut stems = vec![];
        if o.system_audio {
            stems.push(super::audio::Stem::start(
                super::audio::Source::System {
                    exclude_own_audio: o.exclude_own_audio,
                },
                clock.clone(),
                &o.output.with_file_name("system.wav"),
            )?);
        }
        if let Some(mic) = &o.mic {
            stems.push(super::audio::Stem::start(
                super::audio::Source::Microphone(mic.as_deref()),
                clock.clone(),
                &o.output.with_file_name("mic.wav"),
            )?);
        }
        let spec = crate::media::writer::WriterSpec {
            width: o.size.0 as u32,
            height: o.size.1 as u32,
            fps: o.fps,
            codec: o.codec.into(),
            bitrate: o.bitrate,
            quality: o.quality,
            audio: false,
        };
        let desktop = Some(super::desktop::DesktopGuard::new(
            o.hide_desktop_icons,
            o.hide_dock,
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let frames = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let policy = std::env::var("CAM_GPU_CAPTURE").unwrap_or_default();
        // Keep the classic-window repaint fallback on its established path.
        let native = target.as_ref().filter(|target| {
            matches!(target, screen::Target::Display(..))
                && policy != "off"
                && std::env::var("CAM_ENCODER").as_deref() != Ok("software")
                && spec.codec != crate::project::model::Codec::Prores
        });
        if let Some(target) = native {
            let native_error = Arc::new(Mutex::new(None));
            let state = super::native::State {
                clock: clock.clone(),
                stop: stop.clone(),
                cancelled: cancelled.clone(),
                error: native_error.clone(),
                frames: frames.clone(),
            };
            match super::native::start(target, &o.output, spec.clone(), state) {
                Ok(thread) => {
                    return Ok(Self {
                        output: Some(o.output),
                        started: Instant::now(),
                        description: "Windows GPU capture".into(),
                        clock,
                        stop,
                        error: native_error,
                        frames,
                        thread: Some(thread),
                        stems,
                        desktop,
                        cancelled,
                    });
                }
                Err(error) if policy == "require" => return Err(error),
                Err(error) => eprintln!("Cam GPU capture unavailable; using FFmpeg: {error:#}"),
            }
        }
        let capture = target
            .as_ref()
            .map(|t| super::wgc::Stream::start(t, o.fps, false))
            .transpose()?;
        let mut writer = crate::media::writer::MovieWriter::create(&o.output, &spec)?;
        let end = stop.clone();
        let abort = cancelled.clone();
        let err = error.clone();
        let count = frames.clone();
        let timing = clock.clone();
        let thread = std::thread::spawn(move || {
            use std::sync::atomic::Ordering;
            let result = (|| -> Result<Stats> {
                let pb = writer.buffer()?;
                while !end.load(Ordering::Acquire) {
                    if timing.lock().unwrap().pause.is_some() {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    let now = crate::util::host_now();
                    if timing.lock().unwrap().start.is_none() {
                        timing.lock().unwrap().start = Some(now);
                    }
                    let elapsed = timing.lock().unwrap().elapsed(now);
                    let wanted = (elapsed * o.fps as f64).floor() as u64 + 1;
                    if count.load(Ordering::Relaxed) >= wanted {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        continue;
                    }
                    let image = match &capture {
                        Some(stream) => stream.snapshot()?,
                        None => super::grab::Image {
                            width: spec.width,
                            height: spec.height,
                            bgra: crate::synth::screen_bgra(
                                spec.width,
                                spec.height,
                                elapsed * 1000.0,
                            ),
                        },
                    };
                    let data = if (image.width, image.height) == (spec.width, spec.height) {
                        image.bgra
                    } else {
                        let image =
                            image::RgbaImage::from_raw(image.width, image.height, image.bgra)
                                .context("invalid capture pixels")?;
                        image::imageops::resize(
                            &image,
                            spec.width,
                            spec.height,
                            image::imageops::FilterType::Triangle,
                        )
                        .into_raw()
                    };
                    *pb.bgra.lock().unwrap() = data;
                    writer.push_frame(&pb)?;
                    count.fetch_add(1, Ordering::Release);
                    // Duplicate the latest frame only when capture/encoding missed a slot.
                    while count.load(Ordering::Relaxed) < wanted {
                        writer.push_frame(&pb)?;
                        count.fetch_add(1, Ordering::Release);
                    }
                }
                anyhow::ensure!(!abort.load(Ordering::Acquire), "recording cancelled");
                let n = count.load(Ordering::Acquire);
                anyhow::ensure!(n > 0, "no screen frames captured");
                writer.finish()?;
                Ok(Stats {
                    screen_frames_in: n,
                    video_frames_out: n,
                    ..Default::default()
                })
            })();
            if let Err(e) = &result {
                *err.lock().unwrap() = Some(format!("{e:#}"));
            }
            result
        });
        Ok(Self {
            output: Some(o.output),
            started: Instant::now(),
            description: "Windows capture".into(),
            clock,
            stop,
            error,
            frames,
            thread: Some(thread),
            stems,
            desktop,
            cancelled,
        })
    }
    pub fn pause(&self, now: f64) -> bool {
        let mut c = self.clock.lock().unwrap();
        c.pause(now)
    }
    pub fn resume(&self, now: f64) -> bool {
        let mut c = self.clock.lock().unwrap();
        c.resume(now)
    }
    pub fn is_paused(&self) -> bool {
        self.clock.lock().unwrap().pause.is_some()
    }
    pub fn failed(&self) -> bool {
        self.error().is_some()
    }
    pub fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.stems.iter().find_map(|s| s.error()))
    }
    pub fn t0_secs(&self) -> Option<f64> {
        self.clock.lock().unwrap().start
    }
    pub fn recorded_secs(&self) -> f64 {
        self.clock.lock().unwrap().elapsed(crate::util::host_now())
    }
    pub fn has_frames(&self) -> bool {
        self.frames.load(std::sync::atomic::Ordering::Acquire) > 0
    }
    pub fn poll(&self) -> (Stats, [f32; 2]) {
        (
            Stats {
                video_frames_out: self.frames.load(std::sync::atomic::Ordering::Acquire),
                ..Default::default()
            },
            [0.0; 2],
        )
    }
    pub fn finish(mut self) -> Result<Stats> {
        self.pause(crate::util::host_now());
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        let result = self
            .thread
            .take()
            .context("capture already stopped")?
            .join()
            .map_err(|_| anyhow::anyhow!("capture thread panicked"))??;
        let duration = self.recorded_secs();
        for stem in self.stems.drain(..) {
            stem.finish(duration)?;
        }
        self.desktop.take();
        Ok(result)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
pub fn check_permissions(_o: &Options) -> Result<()> {
    Ok(())
}

pub fn clock(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}
