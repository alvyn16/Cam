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
}
pub struct Handler(Shared);
impl GraphicsCaptureApiHandler for Handler {
    type Flags = Shared;
    type Error = anyhow::Error;
    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        Ok(Self(ctx.flags))
    }
    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<()> {
        let (width, height) = (frame.width(), frame.height());
        let buffer = if let Some(a) = self.0.area {
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
        *self.0.frame.lock().unwrap() = Some(Image {
            width: w,
            height: h,
            bgra,
        });
        Ok(())
    }
    fn on_closed(&mut self) -> Result<()> {
        *self.0.error.lock().unwrap() = Some("capture source closed".into());
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
        let area = match target {
            Target::Display(_, a) => *a,
            _ => None,
        };
        let shared = Shared {
            frame: Arc::new(Mutex::new(None)),
            error: Arc::new(Mutex::new(None)),
            area,
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
        while stream.shared.frame.lock().unwrap().is_none() {
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
