//! Windows media adapter backed by the application's standard media implementation.
use crate::platform::media::{
    MediaBackend, VideoCodec, VideoFileWriter, VideoFrameReader, WriterSpec,
};
use crate::render::VideoFrame;
use anyhow::Result;
use std::path::Path;
pub struct WindowsMediaBackend;
impl WindowsMediaBackend {
    pub fn new() -> Self {
        Self
    }
}
struct Reader(crate::media::VideoReader);
impl VideoFrameReader for Reader {
    fn size(&self) -> (u32, u32) {
        self.0.size
    }
    fn fps(&self) -> f64 {
        self.0.fps
    }
    fn duration_ms(&self) -> f64 {
        self.0.duration_ms
    }
    fn read_next_frame(&mut self) -> Result<Option<(f64, Vec<u8>)>> {
        Ok(self.0.next_frame().map(|f| (f.pts_ms, f.bgra)))
    }
}
struct Writer {
    movie: crate::media::writer::MovieWriter,
    audio: Vec<[f32; 2]>,
}
impl VideoFileWriter for Writer {
    fn append_frame(&mut self, _pts_ms: f64, frame: &VideoFrame) -> Result<()> {
        let pb = self.movie.buffer()?;
        crate::media::writer::fill_bgra(&pb, frame.bgra, frame.width, frame.height, frame.stride);
        self.movie.push_frame(&pb)
    }
    fn append_audio(&mut self, pts_ms: f64, samples: &[f32], channels: u16) -> Result<()> {
        anyhow::ensure!(channels > 0, "audio channels cannot be zero");
        let offset = (pts_ms.max(0.0) * 48.0).round() as usize;
        let frames = samples.len() / channels as usize;
        if self.audio.len() < offset + frames {
            self.audio.resize(offset + frames, [0.0; 2]);
        }
        for (slot, frame) in self.audio[offset..offset + frames]
            .iter_mut()
            .zip(samples.chunks_exact(channels as usize))
        {
            *slot = [frame[0], *frame.get(1).unwrap_or(&frame[0])];
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<()> {
        let mut this = *self;
        if !this.audio.is_empty() {
            this.movie.set_soundtrack(std::sync::Arc::new(this.audio));
        }
        this.movie.finish()
    }
}
impl MediaBackend for WindowsMediaBackend {
    fn open_reader(&self, path: &Path) -> Result<Box<dyn VideoFrameReader>> {
        Ok(Box::new(Reader(crate::media::VideoReader::open(path)?)))
    }
    fn create_writer(&self, path: &Path, s: &WriterSpec) -> Result<Box<dyn VideoFileWriter>> {
        let codec = match s.codec {
            VideoCodec::H264 => crate::project::model::Codec::H264,
            VideoCodec::Hevc => crate::project::model::Codec::Hevc,
            VideoCodec::ProRes => crate::project::model::Codec::Prores,
        };
        Ok(Box::new(Writer {
            movie: crate::media::writer::MovieWriter::create(
                path,
                &crate::media::writer::WriterSpec {
                    width: s.width,
                    height: s.height,
                    fps: s.fps,
                    codec,
                    bitrate: s.bitrate as u64,
                    quality: Some(0.95),
                    audio: true,
                },
            )?,
            audio: vec![],
        }))
    }
}
