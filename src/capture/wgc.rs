//! Windows Graphics Capture frames, paced independently by each consumer.
use super::{grab::Image, screen::Target};
use anyhow::{Context as _, Result};
use std::sync::{Arc, Mutex};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::*,
};
#[derive(Clone)]
pub struct Shared {
    frame: Arc<Mutex<Option<Image>>>,
    error: Arc<Mutex<Option<String>>>,
    area: Option<crate::engine::geom::Rect>,
    gpu: bool,
    gpu_frame: Arc<Mutex<Option<Arc<GpuImage>>>>,
}
pub struct GpuImage {
    pub texture: windows62::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
}
pub struct Handler {
    shared: Shared,
    pool: Vec<Arc<GpuImage>>,
}
impl GraphicsCaptureApiHandler for Handler {
    type Flags = Shared;
    type Error = anyhow::Error;
    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        if ctx.flags.gpu {
            use windows62::{Win32::Graphics::Direct3D10::ID3D10Multithread, core::Interface};
            unsafe {
                let _ = ctx
                    .device_context
                    .cast::<ID3D10Multithread>()?
                    .SetMultithreadProtected(true);
            }
        }
        Ok(Self {
            shared: ctx.flags,
            pool: vec![],
        })
    }
    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<()> {
        let (width, height) = (frame.width(), frame.height());
        if self.shared.gpu {
            use windows62::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};
            let (left, top, w, h) = if let Some(a) = self.shared.area {
                let left = (a.x.max(0.0) as u32).min(width);
                let top = (a.y.max(0.0) as u32).min(height);
                (
                    left,
                    top,
                    (a.w as u32).min(width - left),
                    (a.h as u32).min(height - top),
                )
            } else {
                (0, 0, width, height)
            };
            anyhow::ensure!(w > 0 && h > 0, "capture source has empty bounds");
            let available = self
                .pool
                .iter()
                .find(|image| {
                    image.width == w && image.height == h && Arc::strong_count(image) == 1
                })
                .cloned();
            let image = match available {
                Some(image) => image,
                None => {
                    self.pool.retain(|image| Arc::strong_count(image) > 1);
                    // Consumers keep at most a current and latest frame. Under
                    // load retain the latest image instead of blocking WGC.
                    if self.pool.len() >= 4 {
                        return Ok(());
                    }
                    let mut texture = None;
                    unsafe {
                        frame.device().CreateTexture2D(
                            &D3D11_TEXTURE2D_DESC {
                                Width: w,
                                Height: h,
                                MipLevels: 1,
                                ArraySize: 1,
                                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                SampleDesc: DXGI_SAMPLE_DESC {
                                    Count: 1,
                                    Quality: 0,
                                },
                                Usage: D3D11_USAGE_DEFAULT,
                                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0
                                    | D3D11_BIND_RENDER_TARGET.0)
                                    as u32,
                                ..Default::default()
                            },
                            None,
                            Some(&mut texture),
                        )?;
                    }
                    let image = Arc::new(GpuImage {
                        texture: texture.context("capture GPU texture")?,
                        width: w,
                        height: h,
                    });
                    self.pool.push(image.clone());
                    image
                }
            };
            unsafe {
                frame.device_context().CopySubresourceRegion(
                    &image.texture,
                    0,
                    0,
                    0,
                    0,
                    frame.as_raw_texture(),
                    0,
                    Some(&D3D11_BOX {
                        left,
                        top,
                        front: 0,
                        right: left + w,
                        bottom: top + h,
                        back: 1,
                    }),
                );
            }
            *self.shared.gpu_frame.lock().unwrap() = Some(image);
            return Ok(());
        }
        let buffer = if let Some(a) = self.shared.area {
            let (left, top) = (a.x.max(0.0) as u32, a.y.max(0.0) as u32);
            frame.buffer_crop(
                left,
                top,
                (left + a.w as u32).min(width),
                (top + a.h as u32).min(height),
            )?
        } else {
            frame.buffer()?
        };
        let (w, h) = (buffer.width(), buffer.height());
        let mut bytes = vec![];
        let bgra = buffer.as_nopadding_buffer(&mut bytes).to_vec();
        *self.shared.frame.lock().unwrap() = Some(Image {
            width: w,
            height: h,
            bgra,
        });
        Ok(())
    }
    fn on_closed(&mut self) -> Result<()> {
        *self.shared.error.lock().unwrap() = Some("capture source closed".into());
        Ok(())
    }
}
pub struct Stream {
    control: Option<CaptureControl<Handler, anyhow::Error>>,
    shared: Shared,
    window: Option<super::screen::WindowInfo>,
}
impl Stream {
    pub fn start(target: &Target, fps: u32, cursor: bool) -> Result<Self> {
        Self::start_mode(target, fps, cursor, false)
    }
    pub fn start_gpu(target: &Target, fps: u32, cursor: bool) -> Result<Self> {
        Self::start_mode(target, fps, cursor, true)
    }
    fn start_mode(target: &Target, fps: u32, cursor: bool, gpu: bool) -> Result<Self> {
        let area = match target {
            Target::Display(_, a) => *a,
            _ => None,
        };
        let shared = Shared {
            frame: Arc::new(Mutex::new(None)),
            error: Arc::new(Mutex::new(None)),
            area,
            gpu,
            gpu_frame: Default::default(),
        };
        let cursor = if cursor {
            CursorCaptureSettings::WithCursor
        } else {
            CursorCaptureSettings::WithoutCursor
        };
        let interval = MinimumUpdateIntervalSettings::Custom(std::time::Duration::from_secs_f64(
            1.0 / fps.max(1) as f64,
        ));
        let control = match target {
            Target::Display(d, _) => {
                let item = windows_capture::monitor::Monitor::from_raw_hmonitor(d.id as _);
                Handler::start_free_threaded(Settings::new(
                    item,
                    cursor,
                    DrawBorderSettings::WithoutBorder,
                    SecondaryWindowSettings::Default,
                    interval,
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    shared.clone(),
                ))
            }
            Target::Window(w) => {
                let item = windows_capture::window::Window::from_raw_hwnd(w.id as _);
                Handler::start_free_threaded(Settings::new(
                    item,
                    cursor,
                    DrawBorderSettings::WithoutBorder,
                    SecondaryWindowSettings::Include,
                    interval,
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    shared.clone(),
                ))
            }
        }
        .map_err(|e| anyhow::anyhow!("starting Windows Graphics Capture: {e}"))?;
        let stream = Self {
            control: Some(control),
            shared,
            window: match target {
                Target::Window(w) => Some(w.clone()),
                _ => None,
            },
        };
        let start = std::time::Instant::now();
        while if gpu {
            stream.shared.gpu_frame.lock().unwrap().is_none()
        } else {
            stream.shared.frame.lock().unwrap().is_none()
        } {
            if start.elapsed() > std::time::Duration::from_secs(3) {
                anyhow::bail!("Windows Graphics Capture did not deliver a frame");
            }
            if stream.control.as_ref().is_some_and(|c| c.is_finished()) {
                anyhow::bail!("Windows Graphics Capture stopped during initialization");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(stream)
    }
    pub fn snapshot_gpu(&self) -> Result<Arc<GpuImage>> {
        if let Some(error) = self.shared.error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        anyhow::ensure!(
            !self.control.as_ref().is_some_and(|c| c.is_finished()),
            "Windows GPU capture stopped"
        );
        self.shared
            .gpu_frame
            .lock()
            .unwrap()
            .clone()
            .context("no GPU capture frame")
    }
    pub fn snapshot(&self) -> Result<Image> {
        if let Some(e) = self.shared.error.lock().unwrap().clone() {
            anyhow::bail!(e);
        }
        if self.control.as_ref().is_some_and(|c| c.is_finished()) {
            anyhow::bail!("Windows capture stopped");
        }
        let frame = self
            .shared
            .frame
            .lock()
            .unwrap()
            .clone()
            .context("no native frame")?;
        // Classic GDI windows can lose their DWM surface while occluded.
        // Ask the source to paint itself when WGC supplies an empty frame.
        if let Some(window) = &self.window {
            if frame.bgra.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]) {
                if let Ok(painted) = super::grab::snapshot(&Target::Window(window.clone())) {
                    return Ok(painted);
                }
            }
        }
        Ok(frame)
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        if let Some(c) = self.control.take() {
            let _ = c.stop();
        }
    }
}
