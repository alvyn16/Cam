//! Media decoding and hardware encoding abstractions.

use std::path::Path;
use anyhow::Result;
use crate::render::VideoFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    ProRes,
}

#[derive(Clone, Debug)]
pub struct WriterSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: VideoCodec,
    pub bitrate: u32,
}

pub trait VideoFrameReader: Send {
    fn size(&self) -> (u32, u32);
    fn fps(&self) -> f64;
    fn duration_ms(&self) -> f64;
    fn read_next_frame(&mut self) -> Result<Option<(f64, Vec<u8>)>>; // (pts_ms, bgra_data)
}

pub trait VideoFileWriter: Send {
    fn append_frame(&mut self, pts_ms: f64, frame: &VideoFrame) -> Result<()>;
    fn append_audio(&mut self, pts_ms: f64, samples: &[f32], channels: u16) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<()>;
}

pub trait MediaBackend: Send + Sync {
    fn open_reader(&self, path: &Path) -> Result<Box<dyn VideoFrameReader>>;
    fn create_writer(&self, path: &Path, spec: &WriterSpec) -> Result<Box<dyn VideoFileWriter>>;
}
