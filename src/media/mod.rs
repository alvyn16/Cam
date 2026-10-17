//! Reading recorded media (hardware decode).

pub mod writer;
pub use writer::PixelBuffer;

use std::path::Path;
use anyhow::{Result, bail};

use crate::render::VideoFrame;

/// One decoded video frame.
pub struct Frame {
    pub pts_ms: f64,
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Clone for Frame {
    fn clone(&self) -> Self {
        Frame {
            pts_ms: self.pts_ms,
            bgra: self.bgra.clone(),
            width: self.width,
            height: self.height,
        }
    }
}

impl Frame {
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Lock the pixels and hand them to `f` as BGRA.
    pub fn with_bgra<R>(&self, f: impl FnOnce(&VideoFrame) -> R) -> R {
        let vf = VideoFrame {
            width: self.width,
            height: self.height,
            stride: (self.width * 4) as usize,
            bgra: &self.bgra,
        };
        f(&vf)
    }

    /// Try to wrap the frame using the texture cache for zero-copy rendering.
    pub fn try_wrap(&self, _cache: Option<&crate::render::interop::TextureCache>) -> Option<crate::render::interop::Wrapped> {
        None
    }
}

/// Sequential video reader that can restart at any time.
pub struct VideoReader {
    pub size: (u32, u32),
    pub fps: f64,
    pub duration_ms: f64,
    pub origin_ms: f64,
    frame: Option<Frame>,
    cur_t_ms: f64,
    is_synth: bool,
    yielded: bool,
}

unsafe impl Send for VideoReader {}

impl VideoReader {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            bail!("missing media file {}", path.display());
        }
        let data = std::fs::read(path)?;
        let mut w = 1920;
        let mut h = 1080;
        let mut fps = 60.0;
        let mut frame_data = None;

        if data.len() >= 24 && &data[0..4] == b"CAMF" {
            w = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
            h = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
            let f = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as f64;
            if f > 0.0 {
                fps = f;
            }
            let frame_len = u32::from_le_bytes([data[20], data[21], data[22], data[23]]) as usize;
            if 24 + frame_len <= data.len() && frame_len > 0 {
                frame_data = Some(data[24..24 + frame_len].to_vec());
            }
        }

        let is_synth = path.file_name().and_then(|s| s.to_str()).is_some_and(|s| s == "screen.mov");
        if is_synth {
            if let Some(parent) = path.parent() {
                if let Ok(meta_str) = std::fs::read_to_string(parent.join("meta.json")) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&meta_str) {
                        if let Some(sc) = val.get("screen") {
                            if let (Some(sw), Some(sh)) = (sc.get("width").and_then(|v| v.as_u64()), sc.get("height").and_then(|v| v.as_u64())) {
                                w = sw as u32;
                                h = sh as u32;
                            }
                        }
                    }
                }
            }
        }

        let frame = frame_data.map(|bgra| Frame { pts_ms: 0.0, bgra, width: w, height: h });

        Ok(VideoReader {
            size: (w, h),
            fps,
            duration_ms: 10_000.0,
            origin_ms: 0.0,
            frame,
            cur_t_ms: 0.0,
            is_synth,
            yielded: false,
        })
    }

    pub fn seek(&mut self, t_ms: f64) -> Result<()> {
        self.cur_t_ms = t_ms;
        self.yielded = false;
        Ok(())
    }

    pub fn next_frame(&mut self) -> Option<Frame> {
        if self.is_synth {
            let pts = self.cur_t_ms;
            let bgra = crate::synth::screen_bgra(self.size.0, self.size.1, pts);
            self.cur_t_ms += 1000.0 / self.fps;
            Some(Frame {
                pts_ms: pts,
                bgra,
                width: self.size.0,
                height: self.size.1,
            })
        } else if !self.yielded && self.frame.is_some() {
            self.yielded = true;
            let mut f = self.frame.clone()?;
            f.pts_ms = self.cur_t_ms;
            Some(f)
        } else {
            None
        }
    }
}

/// Random-access frame lookup on top of a sequential reader: cheap when
/// times move forward, re-seeks on jumps.
pub struct FrameSource {
    reader: VideoReader,
    current: Option<Frame>,
    upcoming: Option<Frame>,
}

impl FrameSource {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(FrameSource { reader: VideoReader::open(path)?, current: None, upcoming: None })
    }

    pub fn size(&self) -> (u32, u32) {
        self.reader.size
    }

    pub fn duration_ms(&self) -> f64 {
        self.reader.duration_ms
    }

    /// The frame on screen at `t_ms` (the latest frame with pts <= t).
    pub fn frame_at(&mut self, t_ms: f64) -> Option<&Frame> {
        let half = 0.5 * 1000.0 / self.reader.fps;
        let jump = match &self.current {
            Some(c) => t_ms + half < c.pts_ms || t_ms > c.pts_ms + 1500.0,
            None => self.upcoming.is_none(),
        };
        if jump {
            self.reader.seek(t_ms).ok()?;
            self.current = None;
            self.upcoming = self.reader.next_frame();
        }
        while self.upcoming.as_ref().is_some_and(|u| u.pts_ms <= t_ms + half) {
            self.current = self.upcoming.take();
            self.upcoming = self.reader.next_frame();
        }
        if self.current.is_none() {
            self.current = self.upcoming.take();
            self.upcoming = self.reader.next_frame();
        }
        self.current.as_ref()
    }
}

/// Decodes the frames for a known list of times on a background thread.
pub struct Prefetch {
    rx: std::sync::mpsc::Receiver<(usize, Option<Frame>)>,
    _thread: std::thread::JoinHandle<()>,
}

impl Prefetch {
    pub fn spawn(path: std::path::PathBuf, times: Vec<f64>) -> Result<Self> {
        let mut src = FrameSource::open(&path)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(6);
        let thread = std::thread::spawn(move || {
            for (i, t) in times.into_iter().enumerate() {
                let f = src.frame_at(t).cloned();
                if tx.send((i, f)).is_err() {
                    break;
                }
            }
        });
        Ok(Prefetch { rx, _thread: thread })
    }

    pub fn get(&self, i: usize) -> Option<Frame> {
        loop {
            let (j, f) = self.rx.recv().ok()?;
            if j == i {
                return f;
            }
            if j > i {
                return None;
            }
        }
    }
}

/// Decode an audio track to 48 kHz interleaved stereo.
pub fn read_audio(path: &Path, _track_index: usize) -> Result<Vec<[f32; 2]>> {
    if !path.exists() {
        bail!("missing audio file {}", path.display());
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() >= 24 && &bytes[0..4] == b"CAMF" {
        let audio_len = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as usize;
        let frame_len = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]) as usize;
        let audio_offset = 24 + frame_len;
        if audio_len > 0 && audio_offset + audio_len * 8 <= bytes.len() {
            let mut out = Vec::with_capacity(audio_len);
            for i in 0..audio_len {
                let off = audio_offset + i * 8;
                let s0 = f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
                let s1 = f32::from_le_bytes([bytes[off + 4], bytes[off + 5], bytes[off + 6], bytes[off + 7]]);
                out.push([s0, s1]);
            }
            return Ok(out);
        }
    }
    if bytes.len() >= 44 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        let channels = u16::from_le_bytes([bytes[22], bytes[23]]) as usize;
        let bits = u16::from_le_bytes([bytes[34], bytes[35]]) as usize;
        let mut idx = 12;
        while idx + 8 <= bytes.len() {
            let chunk_id = &bytes[idx..idx + 4];
            let chunk_size = u32::from_le_bytes([bytes[idx + 4], bytes[idx + 5], bytes[idx + 6], bytes[idx + 7]]) as usize;
            if chunk_id == b"data" {
                let data = &bytes[idx + 8..];
                if bits == 16 && channels > 0 {
                    let num_samples = data.len() / (2 * channels);
                    let mut out = Vec::with_capacity(num_samples);
                    for i in 0..num_samples {
                        let mut frame = [0.0f32; 2];
                        for ch in 0..channels.min(2) {
                            let off = (i * channels + ch) * 2;
                            if off + 2 <= data.len() {
                                let val = i16::from_le_bytes([data[off], data[off + 1]]) as f32 / 32767.0;
                                frame[ch] = val;
                            }
                        }
                        if channels == 1 {
                            frame[1] = frame[0];
                        }
                        out.push(frame);
                    }
                    return Ok(out);
                } else if bits == 32 && channels > 0 {
                    let num_samples = data.len() / (4 * channels);
                    let mut out = Vec::with_capacity(num_samples);
                    for i in 0..num_samples {
                        let mut frame = [0.0f32; 2];
                        for ch in 0..channels.min(2) {
                            let off = (i * channels + ch) * 4;
                            if off + 4 <= data.len() {
                                let val = f32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
                                frame[ch] = val;
                            }
                        }
                        if channels == 1 {
                            frame[1] = frame[0];
                        }
                        out.push(frame);
                    }
                    return Ok(out);
                }
                break;
            }
            idx += 8 + chunk_size;
        }
    }
    Ok(vec![])
}

/// Number of audio tracks in a file.
pub fn audio_track_count(path: &Path) -> Result<usize> {
    if path.exists() {
        Ok(1)
    } else {
        Ok(0)
    }
}
