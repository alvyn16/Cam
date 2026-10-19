//! Latest-request preview decoding. UI calls only exchange bounded mailboxes.
//! Pipe reads, seeking, process teardown and frame allocation stay on workers.
use super::Frame;
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::Read,
    path::PathBuf,
    process::{Child, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Clone)]
pub struct Track {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: f64,
}

#[derive(Clone, Copy, Debug)]
struct Request {
    id: u64,
    screen_ms: f64,
    camera_ms: Option<f64>,
}

// Small forward updates are normal playback: finish useful work rather than
// cancelling every frame before the decoder can produce anything.
fn compatible(from: Request, to: Request) -> bool {
    let near = |a: f64, b: f64| b >= a && b - a <= 250.0;
    near(from.screen_ms, to.screen_ms)
        && match (from.camera_ms, to.camera_ms) {
            (Some(a), Some(b)) => near(a, b),
            (None, None) => true,
            _ => false,
        }
}

pub struct Decoded {
    pub screen: Option<Arc<Frame>>,
    pub camera: Option<Arc<Frame>>,
    pub error: Option<String>,
    request: Request,
}
#[derive(Default)]
struct Mailbox {
    wanted: Option<Request>,
    pending: Option<Request>,
    ready: Option<Decoded>,
}
#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    changed: Condvar,
    stop: AtomicBool,
}
impl Shared {
    fn cancelled(&self, request: Request) -> bool {
        self.stop.load(Ordering::Acquire)
            || self
                .mailbox
                .lock()
                .unwrap()
                .wanted
                .is_some_and(|wanted| !compatible(request, wanted))
    }
}

pub struct Decoder {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    last: Option<Request>,
    delivered: u64,
}
impl Decoder {
    pub fn new(screen: Track, camera: Option<Track>, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared::default());
        let work = shared.clone();
        let worker = std::thread::spawn(move || {
            let mut screen = Source::new(screen);
            let mut camera = camera.map(Source::new);
            loop {
                let request = {
                    let mut mailbox = work.mailbox.lock().unwrap();
                    while mailbox.pending.is_none() && !work.stop.load(Ordering::Acquire) {
                        mailbox = work.changed.wait(mailbox).unwrap();
                    }
                    if work.stop.load(Ordering::Acquire) {
                        break;
                    }
                    mailbox.pending.take().unwrap()
                };
                let result = (|| -> Result<Decoded> {
                    let image = screen.frame_at(request.screen_ms, &|| work.cancelled(request))?;
                    let cam = match (&mut camera, request.camera_ms) {
                        (Some(source), Some(at)) => {
                            source.frame_at(at, &|| work.cancelled(request))?
                        }
                        _ => None,
                    };
                    Ok(Decoded {
                        screen: image,
                        camera: cam,
                        error: None,
                        request,
                    })
                })();
                if work.cancelled(request) {
                    // Discard partial frame state and kill/reap the obsolete pipes.
                    screen.reset();
                    if let Some(camera) = &mut camera {
                        camera.reset();
                    }
                    continue;
                }
                let decoded = result.unwrap_or_else(|error| Decoded {
                    screen: None,
                    camera: None,
                    error: Some(format!("Preview video: {error:#}")),
                    request,
                });
                let mut mailbox = work.mailbox.lock().unwrap();
                if mailbox
                    .wanted
                    .is_some_and(|wanted| compatible(request, wanted))
                {
                    mailbox.ready = Some(decoded);
                }
                drop(mailbox);
                wake();
            }
        });
        Self {
            shared,
            worker: Some(worker),
            last: None,
            delivered: 0,
        }
    }
    pub fn request(&mut self, screen_ms: f64, camera_ms: Option<f64>) {
        let screen_ms = if screen_ms.is_finite() {
            screen_ms.max(0.0)
        } else {
            0.0
        };
        let camera_ms = camera_ms.map(|t| if t.is_finite() { t.max(0.0) } else { 0.0 });
        if self
            .last
            .is_some_and(|r| r.screen_ms == screen_ms && r.camera_ms == camera_ms)
        {
            return;
        }
        let request = Request {
            id: self.last.map_or(1, |r| r.id + 1),
            screen_ms,
            camera_ms,
        };
        self.last = Some(request);
        let mut mailbox = self.shared.mailbox.lock().unwrap();
        mailbox.wanted = Some(request);
        mailbox.pending = Some(request);
        // Never keep a large stale frame behind newer input.
        if mailbox
            .ready
            .as_ref()
            .is_some_and(|r| !compatible(r.request, request))
        {
            mailbox.ready = None;
        }
        drop(mailbox);
        self.shared.changed.notify_one();
    }
    pub fn take_ready(&mut self) -> Option<Decoded> {
        let decoded = self.shared.mailbox.lock().unwrap().ready.take()?;
        if !self
            .last
            .is_some_and(|wanted| compatible(decoded.request, wanted))
        {
            return None;
        }
        self.delivered = decoded.request.id;
        Some(decoded)
    }
    pub fn pending(&self) -> bool {
        self.last.is_some_and(|r| self.delivered != r.id)
    }
    pub fn shutdown(&mut self) {
        // Pair the stop transition with the condition-variable mutex so a
        // never-used decoder cannot miss its shutdown wakeup.
        let mailbox = self.shared.mailbox.lock().unwrap();
        self.shared.stop.store(true, Ordering::Release);
        drop(mailbox);
        self.shared.changed.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Source {
    track: Track,
    stream: Option<Stream>,
    current: Option<Arc<Frame>>,
    next: Option<Arc<Frame>>,
    native: Option<super::native_decode::Reader>,
    tried_native: bool,
    native_required: bool,
}
impl Source {
    fn new(track: Track) -> Self {
        let policy = std::env::var("CAM_GPU_PREVIEW").unwrap_or_default();
        let tried_native =
            policy == "off" || (track.width.max(track.height) <= 1280 && policy != "require");
        Self {
            track,
            stream: None,
            current: None,
            next: None,
            native: None,
            tried_native,
            native_required: policy == "require",
        }
    }
    fn reset(&mut self) {
        self.stream = None;
        self.current = None;
        self.next = None;
    }
    fn frame_at(&mut self, at: f64, cancelled: &impl Fn() -> bool) -> Result<Option<Arc<Frame>>> {
        if !self.tried_native {
            self.tried_native = true;
            match super::native_decode::Reader::open(&self.track) {
                Ok(reader) => self.native = Some(reader),
                Err(error) if self.native_required => return Err(error),
                Err(error) => eprintln!("Cam GPU preview unavailable; using FFmpeg: {error:#}"),
            }
        }
        ensure!(
            !self.native_required || self.native.is_some(),
            "native GPU preview required but unavailable"
        );
        if let Some(reader) = &mut self.native {
            match reader.frame_at(at, cancelled) {
                Ok(frame) => return Ok(frame),
                Err(error) if cancelled() || self.native_required => return Err(error),
                Err(error) => {
                    eprintln!("Cam GPU preview unavailable; using FFmpeg: {error:#}");
                    self.native = None;
                }
            }
        }
        ensure!(
            self.track.fps.is_finite() && self.track.fps > 0.0,
            "invalid video rate"
        );
        let at = at.clamp(
            0.0,
            (self.track.duration_ms - 1000.0 / self.track.fps).max(0.0),
        );
        let half = 500.0 / self.track.fps;
        let jump = self
            .current
            .as_ref()
            .is_none_or(|f| at + half < f.pts_ms || at > f.pts_ms + 250.0);
        if jump {
            self.reset();
            if cancelled() {
                bail!("seek superseded");
            }
            self.stream = Some(Stream::open(&self.track, at, false)?);
            self.next = self.stream.as_ref().unwrap().read(cancelled)?;
            if self.next.is_none() {
                // Recording clocks and a camera's final packet need not end
                // on the same frame. Seek relative to the actual file end,
                // retaining only its last frame rather than trusting metadata.
                self.stream = None;
                self.stream = Some(Stream::open(&self.track, at, true)?);
                let mut last = None;
                while let Some(frame) = self.stream.as_ref().unwrap().read(cancelled)? {
                    last = Some(frame);
                }
                if let Some(frame) = &mut last {
                    Arc::make_mut(frame).pts_ms = at;
                }
                self.current = last;
                ensure!(self.current.is_some(), "video decoder produced no frame");
                return Ok(self.current.clone());
            }
        }
        while self.next.as_ref().is_some_and(|f| f.pts_ms <= at + half) {
            self.current = self.next.take();
            self.next = self.stream.as_ref().unwrap().read(cancelled)?;
        }
        if self.current.is_none() {
            self.current = self.next.take();
            self.next = self.stream.as_ref().unwrap().read(cancelled)?;
        }
        ensure!(self.current.is_some(), "video decoder produced no frame");
        Ok(self.current.clone())
    }
}

/// Two queued frames at most. Decoder pipes are killed before joining the reader;
/// disconnecting the receiver first also releases a reader blocked on send.
struct Stream {
    child: Child,
    frames: Option<mpsc::Receiver<Result<Arc<Frame>>>>,
    reader: Option<JoinHandle<()>>,
}
impl Stream {
    fn open(track: &Track, at: f64, tail: bool) -> Result<Self> {
        ensure!(
            track.width > 0 && track.height > 0 && track.width <= 16384 && track.height <= 16384,
            "invalid video dimensions"
        );
        let scale = (1280.0 / track.width.max(track.height) as f64).min(1.0);
        let width = ((track.width as f64 * scale).floor() as u32 / 2 * 2).max(2);
        let height = ((track.height as f64 * scale).floor() as u32 / 2 * 2).max(2);
        let mut child = super::ffmpeg::command("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                if tail { "-sseof" } else { "-ss" },
                &format!("{:.6}", if tail { -0.5 } else { at / 1000.0 }),
                "-threads",
                "8",
                "-filter_threads",
                "1",
                "-i",
            ])
            .arg(&track.path)
            .args([
                "-map",
                "0:v:0",
                "-an",
                "-threads",
                "1",
                "-vf",
                &format!("scale={width}:{height}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "bgra",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting preview decoder")?;
        let mut output = child.stdout.take().context("preview decoder pipe")?;
        let (tx, rx) = mpsc::sync_channel(2);
        let fps = track.fps;
        let reader = std::thread::spawn(move || {
            let mut index = 0u64;
            loop {
                let mut bgra = vec![0u8; (width * height * 4) as usize];
                if let Err(error) = output.read_exact(&mut bgra) {
                    if error.kind() != std::io::ErrorKind::UnexpectedEof {
                        let _ = tx.send(Err(error.into()));
                    }
                    break;
                }
                let frame = Arc::new(Frame {
                    pts_ms: at + index as f64 * 1000.0 / fps,
                    width,
                    height,
                    bgra,
                });
                if tx.send(Ok(frame)).is_err() {
                    break;
                }
                index += 1;
            }
        });
        Ok(Self {
            child,
            frames: Some(rx),
            reader: Some(reader),
        })
    }
    fn read(&self, cancelled: &impl Fn() -> bool) -> Result<Option<Arc<Frame>>> {
        loop {
            if cancelled() {
                bail!("seek superseded");
            }
            match self
                .frames
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(10))
            {
                Ok(frame) => return frame.map(Some),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.frames.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    #[test]
    fn required_native_decoder_never_silently_switches_to_software() {
        let mut source = Source::new(Track {
            path: PathBuf::from("unused"),
            width: 1920,
            height: 1080,
            fps: 30.0,
            duration_ms: 1000.0,
        });
        source.tried_native = true;
        source.native_required = true;
        let error = source.frame_at(0.0, &|| false).err().unwrap();
        assert!(error.to_string().contains("required but unavailable"));
    }

    #[test]
    #[ignore = "compares the persistent D3D11 preview with FFmpeg on the selected video and GPU"]
    fn native_gpu_preview_seek_pixels_match_software() {
        let track = if let Some(path) = std::env::var_os("CAM_TEST_VIDEO") {
            let path = PathBuf::from(path);
            let video = super::super::VideoReader::open(&path).unwrap();
            Track {
                path,
                width: video.size.0,
                height: video.size.1,
                fps: video.fps,
                duration_ms: video.duration_ms,
            }
        } else {
            // Exercise the high-resolution path selected in production;
            // small source previews intentionally retain the CPU decoder.
            fixture_sized(1920, 1080)
        };
        let root = std::env::temp_dir().join(format!(
            "cam-native-preview-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut native = super::super::native_decode::Reader::open(&track).unwrap();
        let mut software = Source::new(track.clone());
        software.tried_native = true;
        let mut metrics = vec![];
        for at in [0.0, 1000.0, 2960.0, 7000.0, 2100.0, 7950.0] {
            let started = Instant::now();
            let actual = native.frame_at(at, &|| false).unwrap().unwrap();
            let native_ms = started.elapsed().as_secs_f64() * 1000.0;
            let expected = software.frame_at(at, &|| false).unwrap().unwrap();
            assert_eq!(actual.size(), expected.size());
            let mut squared = 0.0;
            let mut bias = [0.0; 3];
            for (a, b) in actual
                .bgra
                .chunks_exact(4)
                .zip(expected.bgra.chunks_exact(4))
            {
                for channel in 0..3 {
                    let difference = a[channel] as f64 - b[channel] as f64;
                    squared += difference * difference;
                    bias[channel] += difference;
                }
            }
            let pixels = (actual.bgra.len() / 4) as f64;
            let mse = squared / (pixels * 3.0);
            for value in &mut bias {
                *value /= pixels;
            }
            metrics.push(serde_json::json!({"at_ms":at,"decoded_ms":actual.pts_ms,"mse":mse,"bias_bgr":bias,"native_ms":native_ms}));
            for (name, frame) in [("native", &actual), ("software", &expected)] {
                let mut rgba = frame.bgra.clone();
                for p in rgba.chunks_exact_mut(4) {
                    p.swap(0, 2);
                }
                image::save_buffer(
                    root.join(format!("{at}-{name}.png")),
                    &rgba,
                    frame.width,
                    frame.height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
        let cancelled = std::sync::atomic::AtomicUsize::new(0);
        let started = Instant::now();
        assert!(
            native
                .frame_at(2960.0, &|| cancelled.fetch_add(1, Ordering::Relaxed) >= 1)
                .is_err()
        );
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "native read cancellation blocked"
        );
        let after = native.frame_at(0.0, &|| false).unwrap().unwrap();
        assert!(
            after.pts_ms.abs() < 1.0,
            "stale callback after cancelled seek"
        );
        eprintln!(
            "Native preview: {}; artifacts {}",
            serde_json::to_string(&metrics).unwrap(),
            root.display()
        );
        std::fs::write(
            root.join("result.json"),
            serde_json::to_vec_pretty(&metrics).unwrap(),
        )
        .unwrap();
        assert!(
            metrics
                .iter()
                .all(|metric| metric["mse"].as_f64().unwrap() < 100.0),
            "native preview differs from reference: {}",
            root.display()
        );
    }

    fn fixture() -> Track {
        fixture_sized(128, 72)
    }
    fn fixture_sized(width: u32, height: u32) -> Track {
        let path = std::env::temp_dir().join(format!(
            "cam-scrub-{}-{}.mp4",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = super::super::ffmpeg::command("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size={width}x{height}:rate=24"),
                "-t",
                "8",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "96",
                "-threads",
                "1",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Track {
            path,
            width,
            height,
            fps: 24.0,
            duration_ms: 8000.0,
        }
    }
    fn finish(decoder: &mut Decoder) -> Decoded {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(frame) = decoder.take_ready() {
                assert!(frame.error.is_none(), "{:?}", frame.error);
                if !decoder.pending() {
                    return frame;
                }
            }
            assert!(Instant::now() < until, "final seek never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn latest_seek_wins_without_a_queue_and_final_pixels_match() {
        let track = fixture();
        let mut decoder = Decoder::new(track.clone(), Some(track.clone()), || {});
        let start = Instant::now();
        for i in 0..600 {
            decoder.request(if i % 2 == 0 { 10.0 } else { 6500.0 }, Some(100.0));
        }
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "UI request submission blocked"
        );
        decoder.request(3000.0, Some(1000.0));
        let frame = finish(&mut decoder);
        assert_eq!(frame.request.id, 601);
        let mut reference = super::super::FrameSource::open(&track.path).unwrap();
        assert_eq!(
            frame.screen.unwrap().bgra,
            reference.frame_at(3000.0).unwrap().bgra
        );
        assert_eq!(
            frame.camera.unwrap().bgra,
            reference.frame_at(1000.0).unwrap().bgra
        );
        drop(decoder);
        drop(reference);
        std::fs::remove_file(track.path).unwrap();
    }
    #[test]
    fn forward_playback_is_not_starved_and_shutdown_joins() {
        let track = fixture();
        let owner = Arc::new(());
        let owned = owner.clone();
        let mut decoder = Decoder::new(track.clone(), None, move || {
            let _ = &owned;
        });
        decoder.request(0.0, None);
        finish(&mut decoder);
        let mut received = 0;
        for i in 1..=60 {
            decoder.request(i as f64 * 1000.0 / 60.0, None);
            if let Some(frame) = decoder.take_ready() {
                assert!(frame.error.is_none(), "{:?}", frame.error);
                received += 1;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        // The final frame may already have been consumed inside the loop.
        if decoder.pending() {
            finish(&mut decoder);
        }
        assert!(received > 5, "playback starved: {received}");
        decoder.request(7000.0, None);
        let stop = Instant::now();
        drop(decoder);
        assert!(stop.elapsed() < Duration::from_secs(2));
        assert_eq!(Arc::strong_count(&owner), 1, "decoder worker survived drop");
        std::fs::remove_file(track.path).unwrap();
    }
    #[test]
    fn only_small_forward_requests_can_share_in_flight_work() {
        let a = Request {
            id: 1,
            screen_ms: 1000.0,
            camera_ms: Some(500.0),
        };
        assert!(compatible(
            a,
            Request {
                id: 2,
                screen_ms: 1100.0,
                camera_ms: Some(600.0)
            }
        ));
        assert!(!compatible(
            a,
            Request {
                id: 2,
                screen_ms: 999.0,
                ..a
            }
        ));
        assert!(!compatible(
            a,
            Request {
                id: 2,
                screen_ms: 1300.0,
                ..a
            }
        ));
        assert!(!compatible(
            a,
            Request {
                id: 2,
                camera_ms: None,
                ..a
            }
        ));
    }

    #[test]
    fn idle_shutdown_cannot_miss_the_condition_variable_signal() {
        let track = Track {
            path: PathBuf::from("unused"),
            width: 2,
            height: 2,
            fps: 30.0,
            duration_ms: 1000.0,
        };
        for _ in 0..64 {
            drop(Decoder::new(track.clone(), None, || {}));
        }
    }

    #[test]
    fn shorter_camera_tail_keeps_its_actual_last_frame() {
        let mut track = fixture();
        track.duration_ms = 9000.0;
        let mut decoder = Decoder::new(track.clone(), Some(track.clone()), || {});
        decoder.request(8999.0, Some(8999.0));
        let frame = finish(&mut decoder);
        let mut reference = super::super::FrameSource::open(&track.path).unwrap();
        let expected = &reference.frame_at(7958.0).unwrap().bgra;
        assert_eq!(&frame.screen.unwrap().bgra, expected);
        assert_eq!(&frame.camera.unwrap().bgra, expected);
        drop(decoder);
        drop(reference);
        std::fs::remove_file(track.path).unwrap();
    }
}
