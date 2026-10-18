//! DirectShow camera discovery and capture through the media runtime.
use anyhow::{Context, Result, ensure};
use std::{
    io::Read,
    process::{Child, ChildStdout, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
}
pub fn cameras() -> Vec<Device> {
    let out = crate::media::ffmpeg::command("ffmpeg")
        .args([
            "-hide_banner",
            "-list_devices",
            "true",
            "-f",
            "dshow",
            "-i",
            "dummy",
        ])
        .output();
    let Ok(out) = out else { return vec![] };
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.contains("(video)"))
        .filter_map(|l| {
            let a = l.find('"')?;
            let b = l[a + 1..].find('"')?;
            let name = l[a + 1..a + 1 + b].to_owned();
            Some(Device {
                id: name.clone(),
                name,
            })
        })
        .collect()
}
pub fn microphones() -> Vec<Device> {
    use crate::platform::AudioCaptureBackend;
    crate::platform::windows::WindowsAudioBackend::new()
        .input_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| Device {
            id: d.id,
            name: d.name,
        })
        .collect()
}
pub fn ios_devices() -> Vec<Device> {
    cameras()
        .into_iter()
        .filter(|d| {
            let n = d.name.to_lowercase();
            n.contains("iphone") || n.contains("ipad")
        })
        .collect()
}
pub fn enable_screen_capture_devices() {}
pub fn ios_dimensions(id: &str) -> Option<(usize, usize)> {
    ios_devices()
        .iter()
        .any(|d| d.id == id)
        .then_some((1280, 720))
}
pub fn builtin_mic_off(_name: Option<&str>) -> bool {
    false
}
pub struct PreviewReader {
    pub child: Arc<Mutex<Child>>,
    output: ChildStdout,
    width: u32,
    height: u32,
    first: Option<super::grab::Image>,
}
impl PreviewReader {
    pub fn open(source: &super::preview::Source, max_dim: u32) -> Result<Self> {
        let id = match source {
            super::preview::Source::Camera { id } => id.clone(),
            super::preview::Source::Device { id } => Some(id.clone()),
            _ => anyhow::bail!("not a camera source"),
        };
        let devices = cameras();
        let id = id
            .or_else(|| devices.first().map(|d| d.id.clone()))
            .context("no camera available; connect a DirectShow camera or phone capture device")?;
        ensure!(
            devices.iter().any(|d| d.id == id),
            "camera unavailable: {id}"
        );
        let width = max_dim.min(1280).max(16) / 2 * 2;
        let height = (width * 9 / 16) / 2 * 2;
        let mut child=crate::media::ffmpeg::command("ffmpeg").args(["-v","error","-nostdin","-f","dshow","-i"]).arg(format!("video={id}"))
            .args(["-an","-vf",&format!("scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"),"-r","30","-threads","1","-f","rawvideo","-pix_fmt","bgra","pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("opening DirectShow camera")?;
        let output = child.stdout.take().context("camera output")?;
        let mut reader = Self {
            child: Arc::new(Mutex::new(child)),
            output,
            width,
            height,
            first: None,
        };
        let control = reader.child.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let first = reader.read();
            let result = first.map(|f| {
                reader.first = Some(f);
                reader
            });
            let _ = tx.send(result);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(8)) {
            Ok(result) => result,
            Err(_) => {
                let _ = control.lock().unwrap().kill();
                anyhow::bail!(
                    "camera did not deliver frames; check Windows camera privacy and close other camera applications"
                )
            }
        }
    }
    pub fn read(&mut self) -> Result<super::grab::Image> {
        if let Some(f) = self.first.take() {
            return Ok(f);
        }
        let mut bgra = vec![0; (self.width * self.height * 4) as usize];
        self.output
            .read_exact(&mut bgra)
            .context("camera stopped or Windows denied access")?;
        Ok(super::grab::Image {
            width: self.width,
            height: self.height,
            bgra,
        })
    }
}
impl Drop for PreviewReader {
    fn drop(&mut self) {
        let mut c = self.child.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    }
}
pub struct Camera {
    child: Arc<Mutex<Child>>,
    thread: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<Arc<super::grab::Image>>>>,
}
impl Camera {
    pub fn start_with(
        sel: Option<&str>,
        shared: Arc<super::record::Shared>,
        _is_cam: bool,
    ) -> Result<Self> {
        let mut reader = PreviewReader::open(
            &super::preview::Source::Camera {
                id: sel.map(str::to_owned),
            },
            1280,
        )?;
        let child = reader.child.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let end = stop.clone();
        let latest = Arc::new(Mutex::new(None));
        let frames = latest.clone();
        let thread = std::thread::spawn(move || {
            while !end.load(Ordering::Acquire) {
                match reader.read() {
                    Ok(f) => {
                        shared.with(|s| s.push_video(&f));
                        // One replaceable frame, shared with the recording monitor.
                        // Never open a second camera session or queue preview frames.
                        *frames.lock().unwrap() = Some(Arc::new(f));
                    }
                    Err(e) => {
                        if !end.load(Ordering::Acquire) {
                            *shared.error.lock().unwrap() = Some(format!("{e:#}"));
                        }
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            thread: Some(thread),
            stop,
            latest,
        })
    }
    pub fn take_preview_frame(&self) -> Option<Arc<super::grab::Image>> {
        self.latest.lock().unwrap().take()
    }
    pub fn start_ios(id: &str, shared: Arc<super::record::Shared>) -> Result<Self> {
        Self::start_with(Some(id), shared, false)
    }
    pub fn pause(&self, _now: f64) {} // The shared sink owns the pause-adjusted clock.
    pub fn resume(&self, _now: f64) {}
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.child.lock().unwrap().kill();
    }
}
impl Drop for Camera {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
