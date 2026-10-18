//! Independently decoded video and audio, using FFmpeg.
pub mod encoder;
#[cfg(target_os = "windows")]
pub mod mf_encoder;
pub mod audio_file;
pub mod audio_stream;
pub(crate) mod audio_cache;
pub mod ffmpeg;
pub(crate) mod mux;
pub mod writer;
pub(crate) mod wav;
use crate::render::VideoFrame;
use anyhow::{Context, Result, ensure};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Stdio},
};
pub use writer::PixelBuffer;
#[derive(Clone)]
pub struct Frame {
    pub pts_ms: f64,
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
impl Frame {
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub fn with_bgra<R>(&self, f: impl FnOnce(&VideoFrame) -> R) -> R {
        f(&VideoFrame {
            width: self.width,
            height: self.height,
            stride: (self.width * 4) as usize,
            bgra: &self.bgra,
        })
    }
    pub fn try_wrap(
        &self,
        _cache: Option<&crate::render::interop::TextureCache>,
    ) -> Option<crate::render::interop::Wrapped> {
        None
    }
}
pub struct VideoReader {
    pub size: (u32, u32),
    pub fps: f64,
    pub duration_ms: f64,
    pub origin_ms: f64,
    path: PathBuf,
    child: Option<Child>,
    output: Option<ChildStdout>,
    cur_t_ms: f64,
}
impl VideoReader {
    pub fn open(path: &Path) -> Result<Self> {
        let p = ffmpeg::probe(path)?;
        let stream = p["streams"]
            .as_array()
            .context("no media streams")?
            .iter()
            .find(|v| v["codec_type"] == "video")
            .context("no video stream")?;
        let rate = stream["avg_frame_rate"].as_str().unwrap_or("30/1");
        let fps = rate
            .split_once('/')
            .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(30.0);
        let size = (
            stream["width"].as_u64().context("video width")? as u32,
            stream["height"].as_u64().context("video height")? as u32,
        );
        ensure!(
            size.0 > 0 && size.1 > 0 && size.0 <= 16384 && size.1 <= 16384,
            "invalid video dimensions"
        );
        let duration_ms = stream["duration"]
            .as_str()
            .or(p["format"]["duration"].as_str())
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
            * 1000.0;
        Ok(Self {
            size,
            fps,
            duration_ms,
            origin_ms: 0.0,
            path: path.into(),
            child: None,
            output: None,
            cur_t_ms: 0.0,
        })
    }
    pub fn seek(&mut self, t_ms: f64) -> Result<()> {
        self.close();
        self.cur_t_ms = t_ms.max(0.0);
        let mut child = ffmpeg::command("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-ss",
                &format!("{:.6}", self.cur_t_ms / 1000.0),
                "-i",
            ])
            .arg(&self.path)
            .args([
                "-map", "0:v:0", "-an", "-threads", "1", "-f", "rawvideo", "-pix_fmt", "bgra",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting video decoder")?;
        self.output = child.stdout.take();
        self.child = Some(child);
        Ok(())
    }
    pub fn next_frame(&mut self) -> Option<Frame> {
        if self.child.is_none() {
            self.seek(self.cur_t_ms).ok()?;
        }
        let mut bgra = vec![0; (self.size.0 * self.size.1 * 4) as usize];
        self.output.as_mut()?.read_exact(&mut bgra).ok()?;
        let f = Frame {
            pts_ms: self.cur_t_ms,
            bgra,
            width: self.size.0,
            height: self.size.1,
        };
        self.cur_t_ms += 1000.0 / self.fps;
        Some(f)
    }
    fn close(&mut self) {
        self.output.take();
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
impl Drop for VideoReader {
    fn drop(&mut self) {
        self.close();
    }
}
pub struct FrameSource {
    reader: VideoReader,
    current: Option<Frame>,
    upcoming: Option<Frame>,
}
impl FrameSource {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            reader: VideoReader::open(path)?,
            current: None,
            upcoming: None,
        })
    }
    pub fn size(&self) -> (u32, u32) {
        self.reader.size
    }
    pub fn duration_ms(&self) -> f64 {
        self.reader.duration_ms
    }
    pub fn frame_at(&mut self, t_ms: f64) -> Option<&Frame> {
        let t_ms = t_ms.max(0.0);
        let half = 500.0 / self.reader.fps;
        let jump = self.current.as_ref().map_or(self.upcoming.is_none(), |c| {
            t_ms + half < c.pts_ms || t_ms > c.pts_ms + 1500.0
        });
        if jump {
            self.reader.seek(t_ms).ok()?;
            self.current = None;
            self.upcoming = self.reader.next_frame();
        }
        while self
            .upcoming
            .as_ref()
            .is_some_and(|u| u.pts_ms <= t_ms + half)
        {
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
pub struct Prefetch {
    rx: std::sync::mpsc::Receiver<(usize, Option<Frame>)>,
    _thread: std::thread::JoinHandle<()>,
}
impl Prefetch {
    pub fn spawn(path: PathBuf, times: Vec<f64>) -> Result<Self> {
        let mut src = FrameSource::open(&path)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(6);
        let thread = std::thread::spawn(move || {
            for (i, t) in times.into_iter().enumerate() {
                if tx.send((i, src.frame_at(t).cloned())).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            rx,
            _thread: thread,
        })
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
pub fn read_audio(path: &Path, track_index: usize) -> Result<Vec<[f32; 2]>> {
    let out = ffmpeg::output(
        ffmpeg::command("ffmpeg")
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(path)
            .args([
                "-map",
                &format!("0:a:{track_index}"),
                "-vn",
                "-ac",
                "2",
                "-ar",
                "48000",
                "-f",
                "f32le",
                "pipe:1",
            ]),
    )?;
    Ok(out
        .stdout
        .chunks_exact(8)
        .map(|b| {
            [
                f32::from_le_bytes(b[..4].try_into().unwrap()),
                f32::from_le_bytes(b[4..].try_into().unwrap()),
            ]
        })
        .collect())
}
pub fn audio_track_count(path: &Path) -> Result<usize> {
    Ok(ffmpeg::probe(path)?["streams"]
        .as_array()
        .context("no streams")?
        .iter()
        .filter(|v| v["codec_type"] == "audio")
        .count())
}
