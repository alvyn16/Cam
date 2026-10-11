//! Preview audio: the export soundtrack, mixed in the background, written
//! to a temporary WAV and played with AVPlayer in step with the playhead.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2_av_foundation::AVPlayer;
use objc2_core_media::CMTime;
use objc2_foundation::NSURL;

use crate::capture::mixer::RATE;
use crate::engine::timemap::TimeMap;
use crate::project::model::Project;
use crate::project::recording::Recording;
use crate::util::*;

/// A finished mix: its generation and WAV (None = nothing to hear).
type Mixed = (u64, Option<PathBuf>);

pub struct AudioPreview {
    player: Option<SendBox<Retained<AVPlayer>>>,
    /// WAV the player is reading; deleted when replaced.
    current: Option<PathBuf>,
    pending: Arc<Mutex<Option<Mixed>>>,
    /// Newest requested mix; older background mixes are discarded.
    latest: Arc<AtomicU64>,
    /// Edits arrive per frame while dragging a slider: mix once they settle
    /// (the Instant is when the request is due).
    request: Option<(Instant, Project, Recording, PathBuf)>,
    playing: bool,
}

pub fn write_wav(path: &Path, samples: &[[f32; 2]]) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data_len = (samples.len() * 4) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&(RATE as u32).to_le_bytes())?;
    f.write_all(&(RATE as u32 * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        for c in s {
            f.write_all(&((c.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
        }
    }
    f.flush()
}

impl AudioPreview {
    pub fn new() -> Self {
        AudioPreview {
            player: None,
            current: None,
            pending: Arc::new(Mutex::new(None)),
            latest: Arc::new(AtomicU64::new(0)),
            request: None,
            playing: false,
        }
    }

    /// Re-mix after an edit that affects audio (debounced; see `tick`).
    pub fn rebuild(&mut self, project: &Project, rec: &Recording, package: &Path) {
        self.request = Some((Instant::now() + Duration::from_millis(250), project.clone(), rec.clone(), package.to_path_buf()));
    }

    /// Mix right away (opening a project).
    pub fn rebuild_now(&mut self, project: &Project, rec: &Recording, package: &Path) {
        self.request = Some((Instant::now(), project.clone(), rec.clone(), package.to_path_buf()));
        self.start_due();
    }

    fn start_due(&mut self) {
        if !self.request.as_ref().is_some_and(|r| Instant::now() >= r.0) {
            return;
        }
        let (_, p, r, dir) = self.request.take().unwrap();
        let gen_ = self.latest.fetch_add(1, Ordering::SeqCst) + 1;
        let (pending, latest) = (self.pending.clone(), self.latest.clone());
        std::thread::spawn(move || {
            let stale = || latest.load(Ordering::SeqCst) != gen_;
            if stale() {
                return;
            }
            let map = TimeMap::new(&p.timeline.slices, r.meta.duration_ms);
            let path = crate::export::audio::mix(&p, &r, Some(&dir), &map).and_then(|mix| {
                if stale() {
                    return None;
                }
                let path = std::env::temp_dir().join(format!("glide-preview-{}-{gen_}.wav", std::process::id()));
                write_wav(&path, &mix).ok().map(|_| path)
            });
            let mut slot = pending.lock().unwrap();
            if stale() || slot.as_ref().is_some_and(|(g, _)| *g > gen_) {
                if let Some(p) = path {
                    let _ = std::fs::remove_file(p);
                }
                return;
            }
            if let Some((_, Some(old))) = slot.replace((gen_, path)) {
                let _ = std::fs::remove_file(old);
            }
        });
    }

    fn load_pending(&mut self) {
        let Some((_, path)) = self.pending.lock().unwrap().take() else { return };
        if let Some(p) = &self.player {
            unsafe { p.0.pause() };
        }
        self.player = None;
        if let Some(old) = self.current.take() {
            let _ = std::fs::remove_file(old);
        }
        self.playing = false;
        // No path: the edit left nothing audible (e.g. the only track muted).
        let Some(path) = path else { return };
        let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
        let Some(mtm) = objc2::MainThreadMarker::new() else { return };
        let player = unsafe { AVPlayer::playerWithURL(&url, mtm) };
        self.player = Some(SendBox(player));
        self.current = Some(path);
    }

    pub fn play(&mut self, at_ms: f64) {
        self.load_pending();
        if let Some(p) = &self.player {
            unsafe {
                let t = CMTime::new((at_ms * 1000.0) as i64, 1_000_000);
                let zero = CMTime::new(0, 1);
                p.0.seekToTime_toleranceBefore_toleranceAfter(t, zero, zero);
                p.0.play();
            }
            self.playing = true;
        }
    }

    pub fn pause(&mut self) {
        if let Some(p) = &self.player {
            unsafe { p.0.pause() };
        }
        self.playing = false;
    }

    /// Start a settled re-mix; pick up a finished one while not playing.
    pub fn tick(&mut self) {
        self.start_due();
        if !self.playing {
            self.load_pending();
        }
    }
}

/// Remove preview WAVs left by Glide processes that no longer run (crashes).
pub fn sweep_stale() {
    let Ok(dir) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("glide-preview-") else { continue };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<i32>().ok()) else { continue };
        // SAFETY: signal 0 only checks whether the process exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

impl Drop for AudioPreview {
    fn drop(&mut self) {
        self.latest.fetch_add(1, Ordering::SeqCst);
        if let Some(p) = self.current.take() {
            let _ = std::fs::remove_file(p);
        }
        if let Some((_, Some(p))) = self.pending.lock().unwrap().take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Default for AudioPreview {
    fn default() -> Self {
        Self::new()
    }
}
