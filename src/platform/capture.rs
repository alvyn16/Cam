//! Screen and window capture abstractions.

use anyhow::Result;
use crate::engine::geom::Rect;
use crate::render::VideoFrame;

#[derive(Clone, Debug)]
pub struct DisplayInfo {
    pub id: u32,
    pub points: (i64, i64),
    pub pixels: (usize, usize),
}

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub size: (f64, f64),
}

#[derive(Clone, Debug)]
pub enum CaptureTarget {
    Display { id: u32 },
    Window { id: u32 },
    Area { display: u32, rect: Rect },
    Pattern,
    Device { id: String },
}

pub trait CaptureSession: Send {
    fn stop(&mut self);
}

pub trait ScreenCaptureBackend: Send + Sync {
    fn displays(&self) -> Result<Vec<DisplayInfo>>;
    fn windows(&self) -> Result<Vec<WindowInfo>>;
    fn start_capture(
        &self,
        target: &CaptureTarget,
        fps: u32,
        on_frame: Box<dyn FnMut(&VideoFrame) + Send + 'static>,
    ) -> Result<Box<dyn CaptureSession>>;
}
