//! Input monitoring and cursor capture abstractions.

use anyhow::Result;
use crate::engine::geom::Rect;
use crate::project::recording::{CursorImage, MouseKind};

#[derive(Clone, Copy, Debug)]
pub struct RawMouse {
    pub host: f64,
    pub x: f64,
    pub y: f64,
    pub kind: MouseKind,
    pub button: u8,
    pub cursor: u32,
}

#[derive(Clone, Debug)]
pub struct RawKey {
    pub host: f64,
    pub code: u16,
    pub key: String,
    pub mods: u8,
    pub down: bool,
}

pub struct InputLog {
    pub mouse: Vec<RawMouse>,
    pub keys: Vec<RawKey>,
    pub cursors: Vec<CursorImage>,
    pub bounds: Vec<(f64, Rect)>,
    pub keys_enabled: bool,
}

pub trait InputSession: Send {
    fn finish(self: Box<Self>) -> InputLog;
}

pub trait InputBackend: Send + Sync {
    fn start_monitoring(&self, track_window: Option<u32>, log_keys: bool) -> Result<Box<dyn InputSession>>;
    fn current_cursor_image(&self) -> Option<CursorImage>;
    fn display_bounds(&self, display_id: u32) -> Rect;
}
