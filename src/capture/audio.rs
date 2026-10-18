//! Timestamped stereo stems on the same pause-adjusted clock as video.
use crate::platform::{AudioCaptureBackend, audio::AudioStreamSession};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
};
#[derive(Default)]
pub struct Clock {
    pub start: Option<f64>,
    pub pause: Option<f64>,
    pub paused: f64,
}
impl Clock {
    pub fn elapsed(&self, now: f64) -> f64 {
        self.start.map_or(0.0, |s| {
            (self.pause.unwrap_or(now) - s - self.paused).max(0.0)
        })
    }
}
pub enum Source<'a> {
    Microphone(Option<&'a str>),
    System { exclude_own_audio: bool },
}
pub struct Stem {
    session: Box<dyn AudioStreamSession>,
    spool: Option<Spool>,
}
impl Stem {
    pub fn start(source: Source<'_>, clock: Arc<Mutex<Clock>>, path: &Path) -> Result<Self> {
        let spool = Spool::start(path)?;
        let callback = spool.callback(clock);
        let backend = crate::platform::windows::WindowsAudioBackend::new();
        let session = match source {
            Source::Microphone(id) => backend.start_microphone(id, 48000, 2, callback)?,
            Source::System {
                exclude_own_audio: true,
            } => backend.start_loopback_excluding_self(callback)?,
            Source::System {
                exclude_own_audio: false,
            } => backend.start_system_loopback(48000, 2, callback)?,
        };
        Ok(Self {
            session,
            spool: Some(spool),
        })
    }
    pub fn error(&self) -> Option<String> {
        self.session.error().or_else(|| {
            self.spool
                .as_ref()
                .and_then(|s| s.error.lock().unwrap().clone())
        })
    }
    pub fn finish(mut self, duration: f64) -> Result<()> {
        self.session.stop();
        self.spool
            .take()
            .context("audio already stopped")?
            .finish(duration)
    }
}
impl Drop for Stem {
    fn drop(&mut self) {
        self.session.stop();
    }
}

// Bound sample bytes as well as packet count: WASAPI can deliver many tiny
// packets in a burst. A packet-only limit starves long before its byte budget.
const PACKET_FRAMES: usize = 4096;
const QUEUE_PACKETS: usize = 4096;
const QUEUE_BYTES: usize = 8 * 1024 * 1024;
struct Packet {
    at: u64,
    samples: Vec<f32>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
}
impl Drop for Packet {
    fn drop(&mut self) {
        self.queued
            .fetch_sub(self.samples.len() * 4, std::sync::atomic::Ordering::AcqRel);
    }
}
struct Spool {
    sender: Option<SyncSender<Packet>>,
    worker: Option<std::thread::JoinHandle<Result<crate::media::wav::WaveWriter>>>,
    error: Arc<Mutex<Option<String>>>,
    pending: PathBuf,
    destination: PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    preserve: bool,
    queued: Arc<std::sync::atomic::AtomicUsize>,
}
impl Spool {
    fn start(path: &Path) -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let pending = path.with_extension(format!(
            "{}.{}.pending.wav",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut writer = crate::media::wav::WaveWriter::create(&pending)
            .with_context(|| format!("creating audio stem {}", path.display()))?;
        let (tx, rx) = mpsc::sync_channel::<Packet>(QUEUE_PACKETS);
        let error = Arc::new(Mutex::new(None));
        let errors = error.clone();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancelled = cancel.clone();
        let worker = std::thread::spawn(move || {
            unsafe {
                use windows::Win32::System::Threading::{
                    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
                };
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
            }
            while !cancelled.load(std::sync::atomic::Ordering::Acquire) {
                let packet = match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(packet) => packet,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if let Err(e) = writer.write_at(packet.at, &packet.samples) {
                    *errors.lock().unwrap() = Some(format!("Writing recorded audio: {e:#}"));
                    return Err(e);
                }
            }
            Ok(writer)
        });
        Ok(Self {
            sender: Some(tx),
            worker: Some(worker),
            error,
            pending,
            destination: path.into(),
            cancel,
            preserve: false,
            queued: Default::default(),
        })
    }
    fn callback(&self, clock: Arc<Mutex<Clock>>) -> Box<dyn FnMut(&[f32], f64) + Send> {
        let tx = self.sender.as_ref().unwrap().clone();
        let error = self.error.clone();
        let queued = self.queued.clone();
        Box::new(move |data, pts| {
            let c = clock.lock().unwrap();
            let Some(start) = c.start else { return };
            if c.pause.is_some() || !pts.is_finite() {
                return;
            }
            let at = ((pts - start - c.paused) * 48000.0).round() as i64;
            drop(c);
            let skip = at.saturating_neg().max(0).min((data.len() / 2) as i64) as usize;
            for (i, samples) in data[skip * 2..].chunks(PACKET_FRAMES * 2).enumerate() {
                let frame = at.max(0) as u64 + (i * PACKET_FRAMES) as u64;
                let bytes = samples.len() * 4;
                let reserved = queued.fetch_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |n| (n + bytes <= QUEUE_BYTES).then_some(n + bytes),
                );
                if reserved.is_err() {
                    *error.lock().unwrap() = Some("Audio disk writer cannot keep up; recording stopped to avoid losing audio.".into());
                    return;
                }
                let packet = Packet {
                    at: frame,
                    samples: samples.to_vec(),
                    queued: queued.clone(),
                };
                if let Err(e) = tx.try_send(packet) {
                    let mut error = error.lock().unwrap();
                    if error.is_none() {
                        *error = Some(match e {
                            mpsc::TrySendError::Full(_) => "Audio disk writer cannot keep up; recording stopped to avoid losing audio.".into(),
                            mpsc::TrySendError::Disconnected(_) => "Audio disk writer stopped unexpectedly.".into(),
                        });
                    }
                    return;
                }
            }
        })
    }
    fn finish(mut self, duration: f64) -> Result<()> {
        ensure!(
            duration.is_finite() && duration >= 0.0,
            "invalid audio duration"
        );
        drop(self.sender.take());
        let writer = self
            .worker
            .take()
            .context("audio already stopped")?
            .join()
            .map_err(|_| anyhow::anyhow!("audio writer panicked"))??;
        // Queue overflow is surfaced by Stem::error and stops recording. Keep
        // the captured samples and pad the missing tail, rather than discard
        // all audio from an otherwise recoverable project.
        writer.finish((duration * 48000.0).round() as u64)?;
        self.preserve = true;
        std::fs::rename(&self.pending, &self.destination).with_context(|| {
            format!(
                "saving audio stem {}; recorded audio remains at {}",
                self.destination.display(),
                self.pending.display()
            )
        })?;
        Ok(())
    }
}
impl Drop for Spool {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
        drop(self.sender.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if !self.preserve {
            let _ = std::fs::remove_file(&self.pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cam-stem-{label}-{}-{}.wav",
            std::process::id(),
            crate::util::host_now().to_bits()
        ))
    }
    #[test]
    fn disk_stem_clips_prestart_audio_and_removes_paused_time() {
        let path = path("clock");
        let spool = Spool::start(&path).unwrap();
        let clock = Arc::new(Mutex::new(Clock::default()));
        let mut callback = spool.callback(clock.clone());
        callback(&[0.9; 20], 99.0); // no video yet
        clock.lock().unwrap().start = Some(100.0);
        callback(&[0.9; 20], -f64::MAX); // wholly before video, no integer overflow
        callback(
            &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4],
            100.0 - 2.0 / 48000.0,
        );
        clock.lock().unwrap().pause = Some(100.1);
        callback(&[0.8; 20], 100.1);
        {
            let mut c = clock.lock().unwrap();
            c.pause = None;
            c.paused = 1.0;
        }
        callback(&[0.5, -0.5], 101.0 + 10.0 / 48000.0);
        drop(callback);
        spool.finish(20.0 / 48000.0).unwrap();
        let audio = crate::media::read_audio(&path, 0).unwrap();
        assert_eq!(audio.len(), 20);
        assert_eq!(audio[0], [0.3; 2]);
        assert_eq!(audio[1], [0.4; 2]);
        assert_eq!(audio[10], [0.5, -0.5]);
        assert!(audio[2..10].iter().all(|s| *s == [0.0; 2]));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn cancellation_removes_pending_audio_even_with_live_callback() {
        let path = path("cancel");
        std::fs::write(&path, b"existing voiceover").unwrap();
        let spool = Spool::start(&path).unwrap();
        let pending = spool.pending.clone();
        let callback = spool.callback(Arc::new(Mutex::new(Clock::default())));
        drop(spool);
        assert!(!pending.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing voiceover");
        drop(callback);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn slow_disk_queue_is_bounded_and_surfaces_recording_error() {
        let path = path("queue");
        let (tx, _rx) = mpsc::sync_channel(QUEUE_PACKETS);
        let spool = Spool {
            sender: Some(tx),
            worker: None,
            error: Arc::new(Mutex::new(None)),
            pending: path.clone(),
            destination: path,
            cancel: Default::default(),
            preserve: false,
            queued: Default::default(),
        };
        let clock = Arc::new(Mutex::new(Clock {
            start: Some(100.0),
            ..Default::default()
        }));
        let mut callback = spool.callback(clock);
        for _ in 0..QUEUE_PACKETS {
            callback(&[0.1; 8], 100.0);
        }
        assert!(spool.error.lock().unwrap().is_none());
        callback(&[0.1; 8], 100.0);
        assert!(
            spool
                .error
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains("cannot keep up")
        );
    }
    #[test]
    fn recoverable_capture_failure_keeps_recorded_audio() {
        struct FailedSession;
        impl AudioStreamSession for FailedSession {
            fn stop(&mut self) {}
            fn error(&self) -> Option<String> {
                Some("microphone disconnected".into())
            }
        }
        let path = path("partial");
        let spool = Spool::start(&path).unwrap();
        let mut callback = spool.callback(Arc::new(Mutex::new(Clock {
            start: Some(1.0),
            ..Default::default()
        })));
        callback(&[0.5; 20], 1.0);
        drop(callback);
        *spool.error.lock().unwrap() = Some("Audio disk writer cannot keep up".into());
        let stem = Stem {
            session: Box::new(FailedSession),
            spool: Some(spool),
        };
        assert_eq!(stem.error().as_deref(), Some("microphone disconnected"));
        stem.finish(20.0 / 48000.0).unwrap();
        let audio = crate::media::read_audio(&path, 0).unwrap();
        assert_eq!(audio.len(), 20);
        assert!(audio[..10].iter().all(|s| *s == [0.5; 2]));
        assert!(audio[10..].iter().all(|s| *s == [0.0; 2]));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn packet_bursts_use_the_byte_budget_and_release_it_when_consumed() {
        let path = path("burst");
        let (tx, rx) = mpsc::sync_channel(QUEUE_PACKETS);
        let spool = Spool {
            sender: Some(tx),
            worker: None,
            error: Default::default(),
            pending: path.clone(),
            destination: path,
            cancel: Default::default(),
            preserve: false,
            queued: Default::default(),
        };
        let mut callback = spool.callback(Arc::new(Mutex::new(Clock {
            start: Some(1.0),
            ..Default::default()
        })));
        // A delayed WASAPI callback can drain hundreds of small packets at once.
        for _ in 0..512 {
            callback(&[0.1; 960], 1.0);
        }
        assert!(spool.error.lock().unwrap().is_none());
        assert_eq!(
            spool.queued.load(std::sync::atomic::Ordering::Acquire),
            512 * 960 * 4
        );
        while let Ok(packet) = rx.try_recv() {
            drop(packet);
        }
        assert_eq!(spool.queued.load(std::sync::atomic::Ordering::Acquire), 0);
        let samples = vec![0.1; PACKET_FRAMES * 2];
        for _ in 0..QUEUE_BYTES / (samples.len() * 4) {
            callback(&samples, 1.0);
        }
        assert!(spool.error.lock().unwrap().is_none());
        callback(&samples, 1.0);
        assert!(
            spool
                .error
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains("cannot keep up")
        );
        assert_eq!(
            spool.queued.load(std::sync::atomic::Ordering::Acquire),
            QUEUE_BYTES
        );
        drop(rx);
        assert_eq!(spool.queued.load(std::sync::atomic::Ordering::Acquire), 0);
    }
    #[test]
    fn failed_audio_rename_retains_a_decodable_recovery_file() {
        let destination = path("blocked");
        std::fs::create_dir(&destination).unwrap();
        let marker = destination.join("keep");
        std::fs::write(&marker, b"keep").unwrap();
        let spool = Spool::start(&destination).unwrap();
        let pending = spool.pending.clone();
        assert!(
            spool
                .finish(0.01)
                .unwrap_err()
                .to_string()
                .contains("recorded audio remains")
        );
        assert_eq!(crate::media::read_audio(&pending, 0).unwrap().len(), 480);
        assert_eq!(std::fs::read(&marker).unwrap(), b"keep");
        std::fs::remove_file(pending).unwrap();
        std::fs::remove_file(marker).unwrap();
        std::fs::remove_dir(destination).unwrap();
    }
}
