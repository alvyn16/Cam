//! Writing standard video containers through a streaming FFmpeg encoder.
use super::ffmpeg;
use anyhow::{Context, Result, ensure};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
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
    for y in 0..height as usize {
        dst.extend_from_slice(&bgra[y * src_stride..y * src_stride + row]);
    }
}
pub struct MovieWriter {
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    child: Option<Child>,
    input: Option<ChildStdin>,
    path: PathBuf,
    video: PathBuf,
    audio: PathBuf,
    log: PathBuf,
    soundtrack: Option<Soundtrack>,
}
enum Soundtrack {
    Samples(Arc<Vec<[f32; 2]>>),
    File(PathBuf),
}
impl MovieWriter {
    pub fn create(path: &Path, s: &WriterSpec) -> Result<Self> {
        ensure!(
            s.width > 0 && s.height > 0 && s.fps > 0,
            "invalid video dimensions or frame rate"
        );
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let token = format!(
            "{}.{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        );
        let video = path.with_extension(format!("{token}.video.mov"));
        let audio = path.with_extension(format!("{token}.audio.wav"));
        let log = path.with_extension(format!("{token}.encode.log"));
        let mut cmd = ffmpeg::command("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "bgra",
            "-video_size",
        ])
        .arg(format!("{}x{}", s.width, s.height))
        .args([
            "-framerate",
            &s.fps.to_string(),
            "-i",
            "pipe:0",
            "-an",
            "-threads",
            "2",
        ]);
        use crate::project::model::Codec;
        let hardware = super::encoder::hardware(s.codec, s.width, s.height);
        if let Some(encoder) = hardware {
            cmd.args(["-c:v", encoder, "-pix_fmt", "yuv420p", "-bf", "0"]);
            if encoder.ends_with("nvenc") {
                cmd.args(["-preset", "p4"]);
                if let Some(q) = s.quality {
                    cmd.args([
                        "-rc",
                        "vbr",
                        "-cq",
                        &((1.0 - q.clamp(0.0, 1.0)) * 40.0 + 12.0)
                            .round()
                            .to_string(),
                        "-b:v",
                        "0",
                    ]);
                } else {
                    cmd.args(["-b:v", &s.bitrate.max(100_000).to_string()]);
                }
            } else if let Some(q) = s.quality {
                if encoder.ends_with("qsv") {
                    cmd.args([
                        "-global_quality",
                        &((1.0 - q) * 40.0 + 12.0).round().to_string(),
                    ]);
                } else {
                    cmd.args(["-rc", "cqp", "-qp_i", "18", "-qp_p", "20"]);
                }
            } else {
                cmd.args(["-b:v", &s.bitrate.max(100_000).to_string()]);
            }
            if s.codec == Codec::Hevc {
                cmd.args(["-tag:v", "hvc1"]);
            }
        } else {
            match s.codec {
                Codec::H264 => {
                    cmd.args([
                        "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p",
                    ]);
                }
                Codec::Hevc => {
                    cmd.args([
                        "-c:v",
                        "libx265",
                        "-preset",
                        "ultrafast",
                        "-pix_fmt",
                        "yuv420p",
                        "-x265-params",
                        "log-level=error:pools=2:frame-threads=1",
                        "-tag:v",
                        "hvc1",
                    ]);
                }
                Codec::Prores => {
                    cmd.args([
                        "-c:v",
                        "prores_ks",
                        "-profile:v",
                        "4",
                        "-pix_fmt",
                        "yuva444p10le",
                    ]);
                }
            }
            if s.codec != Codec::Prores {
                if let Some(q) = s.quality {
                    cmd.args([
                        "-crf",
                        &((1.0 - q.clamp(0.0, 1.0)) * 40.0 + 12.0)
                            .round()
                            .to_string(),
                    ]);
                } else {
                    cmd.args(["-b:v", &s.bitrate.max(100_000).to_string()]);
                }
            }
        }
        cmd.args([
            "-color_primaries",
            "bt709",
            "-color_trc",
            "iec61966-2-1",
            "-colorspace",
            "bt709",
        ])
        .arg(&video);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log)?)
            .spawn()
            .context("starting FFmpeg encoder")?;
        let input = child.stdin.take();
        Ok(Self {
            frames: 0,
            width: s.width,
            height: s.height,
            child: Some(child),
            input,
            path: path.into(),
            video,
            audio,
            log,
            soundtrack: None,
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
        self.soundtrack = Some(Soundtrack::Samples(mix));
    }
    /// The caller retains ownership of this standard audio file.
    pub fn set_soundtrack_file(&mut self, path: &Path) {
        self.soundtrack = Some(Soundtrack::File(path.into()));
    }
    pub fn push_frame(&mut self, pb: &PixelBuffer) -> Result<()> {
        let data = pb.bgra.lock().unwrap();
        ensure!(
            data.len() == (self.width * self.height * 4) as usize,
            "invalid video frame length"
        );
        self.input
            .as_mut()
            .context("encoder already closed")?
            .write_all(&data)
            .context("writing video frame")?;
        self.frames += 1;
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        self.finish_cancellable(&|| false)
    }
    fn wait_child(&mut self, cancelled: &dyn Fn() -> bool) -> Result<std::process::ExitStatus> {
        loop {
            ensure!(!cancelled(), "media encoding cancelled");
            if let Some(status) = self.child.as_mut().context("encoder closed")?.try_wait()? {
                return Ok(status);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    fn error_log(&self) -> String {
        let mut bytes = Vec::new();
        if let Ok(file) = std::fs::File::open(&self.log) {
            let _ = file.take(64 * 1024).read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
    pub fn finish_cancellable(mut self, cancelled: &dyn Fn() -> bool) -> Result<()> {
        self.input.take();
        let status = self.wait_child(cancelled)?;
        ensure!(
            status.success() && self.frames > 0,
            "encoding failed: {}",
            self.error_log()
        );
        let audio = if let Some(soundtrack) = &self.soundtrack {
            let audio = match soundtrack {
                Soundtrack::Samples(mix) => {
                    crate::app::player::write_wav(&self.audio, mix)?;
                    &self.audio
                }
                Soundtrack::File(path) => path,
            };
            Some(audio.as_path())
        } else {
            None
        };
        super::mux::publish(&self.video, audio, &self.path, cancelled)
    }
    pub fn cancel(self) {}
}
impl Drop for MovieWriter {
    fn drop(&mut self) {
        self.input.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        for p in [&self.video, &self.audio, &self.log] {
            let _ = std::fs::remove_file(p);
        }
    }
}
