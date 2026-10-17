//! Captions: transcribe the microphone track with Whisper.
//! Results go to `recording/transcript.json`.

use std::path::{Path, PathBuf};
use anyhow::{Context, Result, anyhow, bail};

use crate::capture::mixer;
use crate::project::recording::Word;
use crate::project::{Package, write_json};

const RATE: f64 = 16_000.0;
const MAX_CHUNK_S: f64 = 55.0;
const SPLIT_WINDOW_S: f64 = 8.0;

/// Speech-to-text engine for captions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Whisper(crate::whisper::Model),
}

impl Engine {
    pub fn parse(s: &str) -> Option<Engine> {
        let name = s.strip_prefix("whisper-").unwrap_or(s);
        crate::whisper::Model::parse(name).map(Engine::Whisper)
    }

    pub fn label(self) -> String {
        match self {
            Engine::Whisper(m) => format!("Whisper {}", m.name()),
        }
    }
}

/// Transcribe a package's microphone track. Returns the number of words.
pub fn transcribe_package(pkg: &Package, engine: Engine, progress: &std::sync::Arc<crate::whisper::Progress>) -> Result<usize> {
    let rec = pkg.load_recording()?;
    let mic = rec.meta.mic.as_ref().ok_or_else(|| anyhow!("this recording has no microphone track"))?;
    let stereo = crate::media::read_audio(&pkg.rec(&mic.file), mic.track)?;
    let mono = downsample(&stereo);
    let words = match engine {
        Engine::Whisper(model) => crate::whisper::transcribe(&mono, model, progress)?,
    };
    write_json(&pkg.rec("transcript.json"), &words)?;
    Ok(words.len())
}

/// 48 kHz stereo -> 16 kHz mono (box filter over each group of three).
pub fn downsample(stereo: &[[f32; 2]]) -> Vec<f32> {
    let step = (mixer::RATE / RATE).round() as usize;
    stereo
        .chunks(step)
        .map(|c| c.iter().map(|s| (s[0] + s[1]) * 0.5).sum::<f32>() / c.len() as f32)
        .collect()
}

pub fn split_points(samples: &[f32]) -> Vec<usize> {
    let max = (MAX_CHUNK_S * RATE) as usize;
    let win = (0.1 * RATE) as usize;
    let mut cuts = vec![0];
    let mut start = 0;
    while samples.len() - start > max {
        let hi = start + max;
        let lo = hi - (SPLIT_WINDOW_S * RATE) as usize;
        let mut best = (f32::MAX, hi);
        let mut i = lo;
        while i + win <= hi {
            let e: f32 = samples[i..i + win].iter().map(|s| s * s).sum();
            if e < best.0 {
                best = (e, i + win / 2);
            }
            i += win / 2;
        }
        start = best.1;
        cuts.push(start);
    }
    cuts.push(samples.len());
    cuts
}

pub fn write_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&(RATE as u32).to_le_bytes());
    b.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).with_context(|| format!("writing {}", path.display()))
}

pub fn transcript_path(pkg: &Package) -> PathBuf {
    pkg.rec("transcript.json")
}
