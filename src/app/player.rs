//! Preview audio: the export soundtrack, mixed in the background, written
//! to a temporary WAV and played with AVPlayer in step with the playhead.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2_av_foundation::AVPlayer;
use objc2_core_media::CMTime;
use objc2_foundation::NSURL;

use crate::capture::mixer::RATE;
use crate::engine::timemap::TimeMap;
use crate::project::model::Project;
use crate::project::recording::Recording;
use crate::util::*;

pub struct AudioPreview {
    player: Option<SendBox<Retained<AVPlayer>>>,
    pending: Arc<Mutex<Option<PathBuf>>>,
    generation: u64,
    playing: bool,
}

fn write_wav(path: &Path, samples: &[[f32; 2]]) -> std::io::Result<()> {
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
        AudioPreview { player: None, pending: Arc::new(Mutex::new(None)), generation: 0, playing: false }
    }

    /// Re-mix in the background after an edit that affects audio.
    pub fn rebuild(&mut self, project: &Project, rec: &Recording, package: &Path) {
        self.generation += 1;
        let (p, r, dir, pending, gen_) = (project.clone(), rec.clone(), package.to_path_buf(), self.pending.clone(), self.generation);
        std::thread::spawn(move || {
            let map = TimeMap::new(&p.timeline.slices, r.meta.duration_ms);
            let Some(mix) = crate::export::audio::mix(&p, &r, Some(&dir), &map) else { return };
            let path = std::env::temp_dir().join(format!("glide-preview-{}-{gen_}.wav", std::process::id()));
            if write_wav(&path, &mix).is_ok() {
                *pending.lock().unwrap() = Some(path);
            }
        });
    }

    fn load_pending(&mut self) {
        let Some(path) = self.pending.lock().unwrap().take() else { return };
        let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
        let Some(mtm) = objc2::MainThreadMarker::new() else { return };
        let player = unsafe { AVPlayer::playerWithURL(&url, mtm) };
        self.player = Some(SendBox(player));
        self.playing = false;
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

    /// Pick up a finished re-mix while idle.
    pub fn tick(&mut self) {
        if !self.playing {
            self.load_pending();
        }
    }
}

impl Default for AudioPreview {
    fn default() -> Self {
        Self::new()
    }
}
