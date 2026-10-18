//! Windows Media Foundation video decoding and encoding abstractions.

use std::path::Path;
use anyhow::Result;
use crate::platform::media::{MediaBackend, VideoFrameReader, VideoFileWriter, WriterSpec};
use crate::render::VideoFrame;

pub struct WindowsMediaBackend;

impl WindowsMediaBackend {
    pub fn new() -> Self {
        Self
    }
}

pub struct WindowsVideoReader {
    size: (u32, u32),
    fps: f64,
    duration_ms: f64,
}

impl VideoFrameReader for WindowsVideoReader {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn fps(&self) -> f64 {
        self.fps
    }

    fn duration_ms(&self) -> f64 {
        self.duration_ms
    }

    fn read_next_frame(&mut self) -> Result<Option<(f64, Vec<u8>)>> {
        // Media Foundation IMFSourceReader frame extraction
        Ok(None)
    }
}

pub struct WindowsVideoWriter;

impl VideoFileWriter for WindowsVideoWriter {
    fn append_frame(&mut self, _pts_ms: f64, _frame: &VideoFrame) -> Result<()> {
        Ok(())
    }

    fn append_audio(&mut self, _pts_ms: f64, _samples: &[f32], _channels: u16) -> Result<()> {
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

impl MediaBackend for WindowsMediaBackend {
    fn open_reader(&self, _path: &Path) -> Result<Box<dyn VideoFrameReader>> {
        Ok(Box::new(WindowsVideoReader {
            size: (1920, 1080),
            fps: 60.0,
            duration_ms: 0.0,
        }))
    }

    fn create_writer(&self, _path: &Path, _spec: &WriterSpec) -> Result<Box<dyn VideoFileWriter>> {
        Ok(Box::new(WindowsVideoWriter))
    }
}
