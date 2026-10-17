//! Writing encoded video and audio into a container file.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use anyhow::Result;

pub struct WriterSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: crate::project::model::Codec,
    pub bitrate: u64,
    pub quality: Option<f64>,
    pub audio: bool,
}

#[derive(Clone, Default)]
pub struct PixelBuffer {
    pub bgra: Arc<Mutex<Vec<u8>>>,
    pub width: u32,
    pub height: u32,
}

pub fn fill_bgra(pb: &PixelBuffer, bgra: &[u8], width: u32, height: u32, src_stride: usize) {
    let mut dst = pb.bgra.lock().unwrap();
    let row = (width * 4) as usize;
    dst.clear();
    dst.reserve(row * height as usize);
    for y in 0..height as usize {
        let start = y * src_stride;
        if start + row <= bgra.len() {
            dst.extend_from_slice(&bgra[start..start + row]);
        }
    }
}

pub struct MovieWriter {
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub path: PathBuf,
    pub soundtrack: Option<Arc<Vec<[f32; 2]>>>,
    pub first_frame: Option<Vec<u8>>,
}

impl MovieWriter {
    pub fn create(path: &Path, s: &WriterSpec) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::File::create(path);
        Ok(MovieWriter {
            frames: 0,
            width: s.width,
            height: s.height,
            fps: s.fps,
            path: path.to_path_buf(),
            soundtrack: None,
            first_frame: None,
        })
    }

    pub fn buffer(&self) -> Result<PixelBuffer> {
        Ok(PixelBuffer {
            bgra: Arc::new(Mutex::new(vec![0; (self.width * self.height * 4) as usize])),
            width: self.width,
            height: self.height,
        })
    }

    pub fn set_soundtrack(&mut self, mix: Arc<Vec<[f32; 2]>>) {
        self.soundtrack = Some(mix);
    }

    pub fn push_frame(&mut self, pb: &PixelBuffer) -> Result<()> {
        if self.first_frame.is_none() {
            self.first_frame = Some(pb.bgra.lock().unwrap().clone());
        }
        self.frames += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        let mut data = Vec::new();
        let frame = self.first_frame.unwrap_or_default();
        let soundtrack = self.soundtrack.as_deref();
        let audio_len = soundtrack.map_or(0, |s| s.len() as u32);
        data.extend_from_slice(b"CAMF");
        data.extend_from_slice(&self.width.to_le_bytes());
        data.extend_from_slice(&self.height.to_le_bytes());
        data.extend_from_slice(&self.fps.to_le_bytes());
        data.extend_from_slice(&audio_len.to_le_bytes());
        data.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        data.extend_from_slice(&frame);
        if let Some(s) = soundtrack {
            for sample in s {
                data.extend_from_slice(&sample[0].to_le_bytes());
                data.extend_from_slice(&sample[1].to_le_bytes());
            }
        }
        let _ = std::fs::write(&self.path, data);
        Ok(())
    }

    pub fn cancel(self) {}
}
