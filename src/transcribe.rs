//! Captions: transcribe the microphone track with Whisper.
//! Results go to `recording/transcript.json`.

use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

use crate::capture::mixer;
use crate::project::Package;

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
pub fn transcribe_package(
    pkg: &Package,
    engine: Engine,
    progress: &std::sync::Arc<crate::whisper::Progress>,
) -> Result<usize> {
    use std::sync::atomic::Ordering;
    progress.check()?;
    let rec = pkg.load_recording()?;
    let mic = rec
        .meta
        .mic
        .as_ref()
        .ok_or_else(|| anyhow!("this recording has no microphone track"))?;
    let mut decoder = crate::media::audio_stream::AudioStream::open(
        &pkg.rec(&mic.file),
        mic.track,
        RATE as u32,
        1,
    )?;
    progress.phase.store(1, Ordering::Relaxed);
    let cancelled = || progress.is_cancelled();
    let mut chunks = Chunks::default();
    let mut session = None;
    let mut words = Vec::new();
    let total_ms = rec.meta.duration_ms.max(1.0);
    let Engine::Whisper(model) = engine;
    while let Some((start, samples)) =
        chunks.next(&mut || decoder.next_chunk(&cancelled), &cancelled)?
    {
        if crate::whisper::has_signal(&samples) {
            if session.is_none() {
                session = Some(crate::whisper::Session::new(model, progress)?);
            }
            words.extend(session.as_mut().unwrap().chunk(
                &samples,
                start as f64 / 16.0,
                total_ms,
            )?);
        }
        progress.check()?;
        let done = chunks.offset as f64 / 16.0 / total_ms;
        progress
            .permille
            .store((done.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }
    crate::project::write_json_cancellable(&pkg.rec("transcript.json"), &words, &cancelled)?;
    progress.permille.store(1000, Ordering::Relaxed);
    Ok(words.len())
}

#[derive(Default)]
struct Chunks {
    samples: Vec<f32>,
    offset: u64,
    eof: bool,
}
impl Chunks {
    fn next(
        &mut self,
        source: &mut dyn FnMut() -> Result<Option<Vec<f32>>>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<(u64, Vec<f32>)>> {
        let max = (MAX_CHUNK_S * RATE) as usize;
        while self.samples.len() <= max && !self.eof {
            anyhow::ensure!(!cancelled(), "transcription cancelled");
            if let Some(samples) = source()? {
                anyhow::ensure!(
                    samples.len() <= 8192,
                    "transcription decoder block exceeds its bound"
                );
                self.samples.extend(samples);
            } else {
                self.eof = true;
            }
        }
        anyhow::ensure!(!cancelled(), "transcription cancelled");
        if self.samples.is_empty() {
            return Ok(None);
        }
        let cut = if self.samples.len() > max {
            split_points(&self.samples)[1]
        } else {
            self.samples.len()
        };
        let tail = self.samples.split_off(cut);
        let samples = std::mem::replace(&mut self.samples, tail);
        let start = self.offset;
        self.offset = self
            .offset
            .checked_add(samples.len() as u64)
            .context("transcription duration overflow")?;
        Ok(Some((start, samples)))
    }
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

#[cfg(test)]
mod stream_tests {
    use super::*;
    #[test]
    fn bounded_chunks_preserve_every_sample_and_original_offsets() {
        let source: Vec<f32> = (0..(130 * 16000 + 137))
            .map(|i| {
                if (i / 16000) % 9 == 0 {
                    0.0
                } else {
                    (i as f32 * 0.03).sin() * 0.1
                }
            })
            .collect();
        let mut parts = source.chunks(8192);
        let mut pull = || Ok(parts.next().map(|part| part.to_vec()));
        let mut chunks = Chunks::default();
        let mut recovered = Vec::new();
        let mut count = 0;
        while let Some((offset, samples)) = chunks.next(&mut pull, &|| false).unwrap() {
            assert_eq!(offset as usize, recovered.len());
            assert!(samples.len() <= 880000);
            assert!(chunks.samples.len() <= 8 * 16000 + 8192);
            recovered.extend(samples);
            count += 1;
        }
        assert!(count >= 3);
        assert_eq!(recovered, source);
        assert_eq!(chunks.offset, source.len() as u64);
    }
    #[test]
    fn cancelled_failed_and_oversized_sources_do_not_return_partial_chunks() {
        let mut chunks = Chunks::default();
        assert!(
            chunks
                .next(&mut || Ok(Some(vec![0.0; 8192])), &|| true)
                .is_err()
        );
        assert_eq!(chunks.offset, 0);
        assert!(
            chunks
                .next(&mut || anyhow::bail!("source failed"), &|| false)
                .is_err()
        );
        assert!(
            chunks
                .next(&mut || Ok(Some(vec![0.0; 8193])), &|| false)
                .is_err()
        );
        assert_eq!(chunks.offset, 0);
    }
}
