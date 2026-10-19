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
    pub pauses: Vec<Pause>,
}
pub struct Pause {
    start: f64,
    end: f64,
    total: f64,
}
struct AudioSpan {
    offset: usize,
    end: usize,
    at: u64,
}
impl Clock {
    pub fn elapsed(&self, now: f64) -> f64 {
        self.start.map_or(0.0, |s| {
            (self.pause.unwrap_or(now) - s - self.paused).max(0.0)
        })
    }
    pub fn pause(&mut self, now: f64) -> bool {
        if self.pause.is_some() {
            return false;
        }
        self.pause = Some(now);
        true
    }
    pub fn resume(&mut self, now: f64) -> bool {
        let Some(start) = self.pause.take() else {
            return false;
        };
        self.paused += (now - start).max(0.0);
        self.pauses.push(Pause {
            start,
            end: now.max(start),
            total: self.paused,
        });
        true
    }
    // A delayed packet may straddle pause/resume, or arrive after several
    // transitions. Map its capture time, rather than the UI's current state.
    fn audio_spans(&self, pts: f64, frames: usize) -> Vec<AudioSpan> {
        let Some(start) = self.start else {
            return vec![];
        };
        if !pts.is_finite() {
            return vec![];
        }
        let end = pts + frames as f64 / 48_000.0;
        let mut from = pts.max(start);
        if from >= end {
            return vec![];
        }
        let first = self.pauses.partition_point(|pause| pause.end <= from);
        let mut removed = first.checked_sub(1).map_or(0.0, |i| self.pauses[i].total);
        let mut spans = Vec::new();
        let mut append = |from: f64, until: f64, removed: f64| {
            // Tolerate floating-point error at an exact sample boundary.
            let index =
                |time: f64| (((time - pts) * 48_000.0 - 1e-6).ceil().max(0.0) as usize).min(frames);
            let offset = index(from);
            let end = index(until);
            if offset < end {
                spans.push(AudioSpan {
                    offset,
                    end,
                    at: ((pts + offset as f64 / 48_000.0 - start - removed) * 48_000.0)
                        .round()
                        .max(0.0) as u64,
                });
            }
        };
        for pause in &self.pauses[first..] {
            if pause.start >= end {
                break;
            }
            append(from, pause.start.min(end), removed);
            from = from.max(pause.end);
            removed = pause.total;
            if from >= end {
                break;
            }
        }
        append(from, end.min(self.pause.unwrap_or(end)), removed);
        spans
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
            let spans = c.audio_spans(pts, data.len() / 2);
            drop(c);
            for span in spans {
                for (i, samples) in data[span.offset * 2..span.end * 2]
                    .chunks(PACKET_FRAMES * 2)
                    .enumerate()
                {
                    let frame = span.at + (i * PACKET_FRAMES) as u64;
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
    #[test]
    #[ignore = "compares connected microphone samples with the actual timestamped WAV under screen capture"]
    fn microphone_stem_matches_received_samples_during_screen_capture() {
        let root = std::env::temp_dir().join(format!(
            "cam-mic-source-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let recorder = crate::recorder::Recorder::start(crate::recorder::Options {
            target: crate::recorder::Target::Area {
                display: 0,
                rect: crate::engine::geom::Rect::new(100.0, 100.0, 320.0, 200.0),
            },
            dir: root.clone(),
            name: Some("Screen".into()),
            mic: None,
            system_audio: false,
            fps: 30,
            ..Default::default()
        })
        .unwrap();
        let path = root.join("microphone.wav");
        let spool = Spool::start(&path).unwrap();
        let clock = Arc::new(Mutex::new(Clock::default()));
        let mut sink = spool.callback(clock.clone());
        let packets = Arc::new(Mutex::new(Vec::new()));
        let received = packets.clone();
        let mut session = crate::platform::windows::WindowsAudioBackend::new()
            .start_microphone(
                None,
                48000,
                2,
                Box::new(move |samples, at| {
                    received.lock().unwrap().push((at, samples.to_vec()));
                    sink(samples, at);
                }),
            )
            .unwrap();
        let start = crate::util::host_now();
        clock.lock().unwrap().start = Some(start);
        packets.lock().unwrap().clear();
        eprintln!("Microphone source capture ready: {}", root.display());
        std::thread::sleep(std::time::Duration::from_secs(16));
        session.stop();
        assert!(session.error().is_none(), "{:?}", session.error());
        let frames = ((crate::util::host_now() - start) * 48000.0).round() as usize;
        spool.finish(frames as f64 / 48000.0).unwrap();
        recorder.stop().unwrap();
        let packets = packets.lock().unwrap();
        let mut expected = vec![[0.0; 2]; frames];
        let mut native = Vec::new();
        let (mut gaps, mut overlaps) = (0usize, 0usize);
        let mut previous = None;
        for (at, data) in packets.iter() {
            if *at >= start {
                if let Some(end) = previous {
                    let delta: f64 = at - end;
                    gaps += usize::from(delta > 1.5 / 48000.0);
                    overlaps += usize::from(delta < -1.5 / 48000.0);
                }
                previous = Some(at + data.len() as f64 / 96000.0);
            }
            let position = ((at - start) * 48000.0).round() as i64;
            for (i, sample) in data.chunks_exact(2).enumerate() {
                let frame = [sample[0], sample[1]];
                native.push(frame);
                if let Some(out) = expected.get_mut((position + i as i64) as usize) {
                    *out = frame;
                }
            }
        }
        let actual = crate::media::read_audio(&path, 0).unwrap();
        let zero_runs = |samples: &[[f32; 2]]| {
            let (mut run, mut count, mut total) = (0usize, 0usize, 0usize);
            for sample in samples {
                if *sample == [0.0; 2] {
                    run += 1;
                } else {
                    if run >= 24 {
                        count += 1;
                        total += run;
                    }
                    run = 0;
                }
            }
            (count, total)
        };
        let metrics = serde_json::json!({"packets":packets.len(),"capture_gaps":gaps,"capture_overlaps":overlaps,"source_zero_runs_and_frames":zero_runs(&native),"saved_zero_runs_and_frames":zero_runs(&actual)});
        std::fs::write(root.join("result.json"), metrics.to_string()).unwrap();
        eprintln!(
            "Microphone source comparison: {metrics}; artifacts {}",
            root.display()
        );
        assert!(packets.len() > 100);
        assert_eq!(
            &actual[4800..],
            &expected[4800..],
            "saved microphone differs from received samples"
        );
        assert_eq!(
            (gaps, overlaps),
            (0, 0),
            "capture lost packets under screen-recording load"
        );
    }
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
        clock.lock().unwrap().pause(100.0 + 2.0 / 48000.0);
        callback(&[0.8; 20], 100.1);
        {
            let mut c = clock.lock().unwrap();
            c.resume(101.0 + 2.0 / 48000.0);
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
    fn delayed_packets_crossing_multiple_pauses_keep_only_active_samples() {
        let path = path("delayed-pause");
        let mut clock = Clock {
            start: Some(10.0),
            ..Default::default()
        };
        clock.pause(10.0 + 4.0 / 48000.0);
        clock.resume(10.0 + 8.0 / 48000.0);
        clock.pause(10.0 + 12.0 / 48000.0);
        clock.resume(10.0 + 16.0 / 48000.0);
        let spool = Spool::start(&path).unwrap();
        let mut callback = spool.callback(Arc::new(Mutex::new(clock)));
        let input: Vec<_> = (0..24).map(|i| [0.1 + i as f32 * 0.01; 2]).collect();
        let flat: Vec<_> = input.iter().flatten().copied().collect();
        callback(&flat, 10.0); // delivered only after both resumes
        drop(callback);
        spool.finish(16.0 / 48000.0).unwrap();
        let actual = crate::media::read_audio(&path, 0).unwrap();
        let expected: Vec<_> = input[..4]
            .iter()
            .chain(&input[8..12])
            .chain(&input[16..])
            .copied()
            .collect();
        assert_eq!(
            actual, expected,
            "paused samples must neither leak nor overwrite earlier speech"
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn packet_delivered_while_paused_keeps_its_pre_pause_samples() {
        let path = path("pause-boundary");
        let mut clock = Clock {
            start: Some(10.0),
            ..Default::default()
        };
        clock.pause(10.0 + 4.0 / 48000.0);
        let spool = Spool::start(&path).unwrap();
        let mut callback = spool.callback(Arc::new(Mutex::new(clock)));
        callback(&[0.25; 16], 10.0);
        drop(callback);
        spool.finish(4.0 / 48000.0).unwrap();
        assert_eq!(
            crate::media::read_audio(&path, 0).unwrap(),
            vec![[0.25; 2]; 4]
        );
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
