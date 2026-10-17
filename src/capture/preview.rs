//! Windows live preview implementation using Windows Graphics Capture.

use anyhow::Result;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

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
}

impl LivePreview {
    pub fn start(
        _source: &Source,
        _max_dim: u32,
        _fps: u32,
        _audio: bool,
        _on_frame: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        Ok(LivePreview {
            level: Arc::new(AtomicU32::new(0)),
            target: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }

    pub fn set_target(&self, w: u32, h: u32) {
        self.target.store((w as u64) << 32 | h as u64, Ordering::Relaxed);
    }

    pub fn take_frame(&self) -> Option<Frame> {
        None
    }

    pub fn take_buffer(&self) -> Option<crate::media::PixelBuffer> {
        None
    }
}
