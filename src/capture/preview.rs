//! Live source frames from the same native capture implementation as recording.
use anyhow::{Context, Result};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Display { id: u32 },
    Area { display: u32, rect: [f64; 4] },
    Window { id: u32 },
    Camera { id: Option<String> },
    Device { id: String },
}
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
pub struct LivePreview {
    level: Arc<AtomicU32>,
    target: Arc<AtomicU64>,
    frame: Arc<Mutex<Option<Frame>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    audio: Option<Box<dyn crate::platform::audio::AudioStreamSession>>,
    camera_child: Option<Arc<Mutex<std::process::Child>>>,
}
impl LivePreview {
    pub fn start(
        source: &Source,
        max_dim: u32,
        fps: u32,
        audio: bool,
        on_frame: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        use crate::{capture::screen, platform::AudioCaptureBackend};
        let target = match source {
            Source::Display { id } | Source::Area { display: id, .. } => {
                let d = screen::displays()?
                    .into_iter()
                    .find(|d| *id == 0 || d.id == *id)
                    .context("display unavailable")?;
                let area = if let Source::Area { rect, .. } = source {
                    Some(crate::engine::geom::Rect::new(
                        rect[0], rect[1], rect[2], rect[3],
                    ))
                } else {
                    None
                };
                Some(screen::Target::Display(d, area))
            }
            Source::Window { id } => Some(screen::Target::Window(screen::find_window(
                &id.to_string(),
            )?)),
            _ => None,
        };
        let camera = if target.is_none() {
            Some(super::camera::PreviewReader::open(source, max_dim)?)
        } else {
            None
        };
        let capture = target
            .as_ref()
            .map(|t| super::wgc::Stream::start(t, fps, true))
            .transpose()?;
        let camera_child = camera.as_ref().map(|c| c.child.clone());
        let first = if let Some(c) = &capture {
            Some(c.snapshot()?)
        } else {
            None
        };
        let level = Arc::new(AtomicU32::new(0));
        let audio = if audio {
            let l = level.clone();
            Some(
                crate::platform::windows::WindowsAudioBackend::new().start_system_loopback(
                    48000,
                    2,
                    Box::new(move |s, _| {
                        let p = s.iter().fold(0.0f32, |p, s| p.max(s.abs()));
                        l.fetch_max(p.to_bits(), Ordering::Relaxed);
                    }),
                )?,
            )
        } else {
            None
        };
        let frame = Arc::new(Mutex::new(None));
        let output = frame.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let end = stop.clone();
        let dimensions = Arc::new(AtomicU64::new(0));
        let thread = std::thread::spawn(move || {
            let mut first = first;
            let mut camera = camera;
            while !end.load(Ordering::Acquire) {
                let now = std::time::Instant::now();
                let image = if let Some(f) = first.take() {
                    Ok(f)
                } else if let Some(c) = &capture {
                    c.snapshot()
                } else {
                    camera
                        .as_mut()
                        .context("no preview source")
                        .and_then(|c| c.read())
                };
                match image {
                    Ok(f) => {
                        let mut rgba = f.bgra;
                        for p in rgba.chunks_exact_mut(4) {
                            p.swap(0, 2);
                        }
                        let scale = if max_dim == 0 {
                            1.0
                        } else {
                            (max_dim as f64 / f.width.max(f.height) as f64).min(1.0)
                        };
                        let (w, h) = (
                            (f.width as f64 * scale).round().max(1.0) as u32,
                            (f.height as f64 * scale).round().max(1.0) as u32,
                        );
                        if (w, h) != (f.width, f.height) {
                            if let Some(img) = image::RgbaImage::from_raw(f.width, f.height, rgba) {
                                rgba = image::imageops::resize(
                                    &img,
                                    w,
                                    h,
                                    image::imageops::FilterType::Triangle,
                                )
                                .into_raw();
                            } else {
                                break;
                            }
                        }
                        *output.lock().unwrap() = Some(Frame {
                            width: w,
                            height: h,
                            rgba,
                        });
                        on_frame();
                    }
                    Err(e) => {
                        if !end.load(Ordering::Acquire) {
                            eprintln!("Cam preview: {e:#}");
                        }
                        break;
                    }
                }
                std::thread::sleep(
                    std::time::Duration::from_secs_f64(1.0 / fps.max(1) as f64)
                        .saturating_sub(now.elapsed()),
                );
            }
        });
        Ok(Self {
            level,
            target: dimensions,
            frame,
            stop,
            thread: Some(thread),
            audio,
            camera_child,
        })
    }
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }
    pub fn set_target(&self, w: u32, h: u32) {
        self.target
            .store((w as u64) << 32 | h as u64, Ordering::Relaxed);
    }
    pub fn take_frame(&self) -> Option<Frame> {
        self.frame.lock().unwrap().take()
    }
    // This capture source supplies CPU frames; only take_frame consumes them.
    pub fn take_buffer(&self) -> Option<crate::media::PixelBuffer> {
        None
    }
}
impl Drop for LivePreview {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.audio.take();
        if let Some(c) = &self.camera_child {
            let _ = c.lock().unwrap().kill();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
