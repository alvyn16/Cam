//! Find a test tone in audio: how long it's present and how loud, measured
//! in 50 ms blocks so silence before and after doesn't hide it. A block has
//! the tone when that frequency stands well above its neighbours, so music
//! or speech playing at the same time (louder, even) doesn't either. Used by
//! `cam audio-check` and the self-tests.

use crate::capture::mixer::RATE;
use crate::export::audio::Stereo;

/// A track's level and how much of it is the tone.
#[derive(Clone, Copy, Debug, Default)]
pub struct Level {
    pub seconds: f64,
    /// Whole-track RMS, dBFS.
    pub rms_db: f32,
    /// Seconds where the tone stands out (10x its neighbouring frequencies)
    /// and is above -60 dBFS.
    pub tone_seconds: f64,
    /// RMS of the tone where present, dBFS (-inf if never).
    pub tone_db: f32,
}

const BLOCK: usize = 2400;

struct Analyzer {
    hz: f64,
    frames: u64,
    energy: f64,
    tone_blocks: u64,
    tone_energy: f64,
    partial: [f32; BLOCK],
    used: usize,
}
impl Analyzer {
    fn new(hz: f64) -> Self {
        Self {
            hz,
            frames: 0,
            energy: 0.0,
            tone_blocks: 0,
            tone_energy: 0.0,
            partial: [0.0; BLOCK],
            used: 0,
        }
    }
    fn push(&mut self, left: f32, right: f32) {
        let mono = (left + right) * 0.5;
        self.energy += (mono as f64) * (mono as f64);
        self.frames += 1;
        self.partial[self.used] = mono;
        self.used += 1;
        if self.used == BLOCK {
            let tone = tone_power(&self.partial, self.hz);
            let around = [-220.0, -160.0, -100.0, 100.0, 160.0, 220.0]
                .iter()
                .map(|delta| tone_power(&self.partial, self.hz + delta))
                .sum::<f64>()
                / 6.0;
            if tone > 1e-6 && tone > 10.0 * around {
                self.tone_blocks += 1;
                self.tone_energy += tone;
            }
            self.used = 0;
        }
    }
    fn finish(self) -> Level {
        Level {
            seconds: self.frames as f64 / RATE,
            rms_db: db(self.energy / self.frames.max(1) as f64),
            tone_seconds: self.tone_blocks as f64 * BLOCK as f64 / RATE,
            tone_db: if self.tone_blocks > 0 {
                db(self.tone_energy / self.tone_blocks as f64)
            } else {
                f32::NEG_INFINITY
            },
        }
    }
}

/// Analyze a complete file track with fixed sample memory. The last partial
/// block contributes to RMS/duration; tone detection retains 50 ms resolution.
pub fn analyze_file(
    path: &std::path::Path,
    track: usize,
    hz: f64,
    cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<Level> {
    anyhow::ensure!(
        hz.is_finite() && hz > 0.0 && hz < RATE / 2.0,
        "tone frequency must be between 0 and 24000 Hz"
    );
    anyhow::ensure!(!cancelled(), "audio analysis cancelled");
    let mut decoder = crate::media::audio_stream::AudioStream::open(path, track, RATE as u32, 2)?;
    let mut analyzer = Analyzer::new(hz);
    while let Some(chunk) = decoder.next_chunk(cancelled)? {
        for frame in chunk.chunks_exact(2) {
            analyzer.push(frame[0], frame[1]);
        }
    }
    Ok(analyzer.finish())
}

fn db(mean_square: f64) -> f32 {
    (10.0 * mean_square.max(1e-20).log10()) as f32
}

/// Mean-square of the `hz` component of `x` (Goertzel).
fn tone_power(x: &[f32], hz: f64) -> f64 {
    let w = std::f64::consts::TAU * hz / RATE;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s0 = v as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let mag2 = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    2.0 * mag2 / (x.len() as f64 * x.len() as f64)
}

pub fn analyze(samples: &[Stereo], hz: f64) -> Level {
    let mono: Vec<f32> = samples.iter().map(|s| (s[0] + s[1]) * 0.5).collect();
    let total =
        mono.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / mono.len().max(1) as f64;
    let (mut blocks, mut tone_energy) = (0usize, 0.0f64);
    for b in mono.chunks_exact(BLOCK) {
        let tone = tone_power(b, hz);
        let around = [-220.0, -160.0, -100.0, 100.0, 160.0, 220.0]
            .iter()
            .map(|d| tone_power(b, hz + d))
            .sum::<f64>()
            / 6.0;
        if tone > 1e-6 && tone > 10.0 * around {
            blocks += 1;
            tone_energy += tone;
        }
    }
    Level {
        seconds: samples.len() as f64 / RATE,
        rms_db: db(total),
        tone_seconds: blocks as f64 * BLOCK as f64 / RATE,
        tone_db: if blocks > 0 {
            db(tone_energy / blocks as f64)
        } else {
            f32::NEG_INFINITY
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, amp: f32, secs: f64) -> Vec<Stereo> {
        (0..(secs * RATE) as usize)
            .map(|i| {
                let v = amp * (std::f64::consts::TAU * hz * i as f64 / RATE).sin() as f32;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn finds_the_tone_and_its_level() {
        // 1 s silence, 2 s of 880 Hz at 0.1, 1 s of 440 Hz.
        let mut s = vec![[0.0; 2]; RATE as usize];
        s.extend(sine(880.0, 0.1, 2.0));
        s.extend(sine(440.0, 0.1, 1.0));
        let l = analyze(&s, 880.0);
        assert!((l.tone_seconds - 2.0).abs() < 0.11, "{l:?}");
        // A 0.1 sine is -23 dBFS RMS.
        assert!((l.tone_db + 23.0).abs() < 0.5, "{l:?}");
        assert!(analyze(&s, 1000.0).tone_seconds < 0.1);
        assert_eq!(analyze(&vec![[0.0; 2]; 48_000], 880.0).tone_seconds, 0.0);
    }

    #[test]
    fn hears_the_tone_under_louder_sound() {
        // The tone at 0.05 under noise and a chord, both much louder.
        let mut seed = 1u32;
        let s: Vec<Stereo> = (0..(2.0 * RATE) as usize)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
                let t = i as f64 / RATE;
                let chord = [261.6, 329.6, 392.0]
                    .iter()
                    .map(|f| (std::f64::consts::TAU * f * t).sin())
                    .sum::<f64>() as f32
                    * 0.15;
                let v =
                    0.05 * (std::f64::consts::TAU * 1234.0 * t).sin() as f32 + noise * 0.4 + chord;
                [v, v]
            })
            .collect();
        let l = analyze(&s, 1234.0);
        assert!(l.tone_seconds > 1.9, "{l:?}");
        assert!(l.rms_db > l.tone_db + 6.0, "the rest is louder: {l:?}");
        assert!(
            analyze(&s, 1500.0).tone_seconds < 0.1,
            "no tone where there isn't one"
        );
    }

    #[test]
    fn streamed_analysis_matches_reference_across_blocks_and_partial_tail() {
        let mut samples = sine(880.0, 0.1, 1.071);
        samples.extend(sine(440.0, 0.07, 0.113));
        samples.extend(vec![[0.0; 2]; 157]);
        let expected = analyze(&samples, 880.0);
        let mut actual = Analyzer::new(880.0);
        for block in samples.chunks(137) {
            for frame in block {
                actual.push(frame[0], frame[1]);
            }
        }
        let actual = actual.finish();
        assert_eq!(actual.seconds, expected.seconds);
        assert_eq!(actual.rms_db, expected.rms_db);
        assert_eq!(actual.tone_seconds, expected.tone_seconds);
        assert_eq!(actual.tone_db, expected.tone_db);

        let path = std::env::temp_dir().join(format!(
            "cam-stream-analysis-{}-{}.wav",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        crate::app::player::write_wav(&path, &samples).unwrap();
        let decoded = crate::media::read_audio(&path, 0).unwrap();
        let expected = analyze(&decoded, 880.0);
        let actual = analyze_file(&path, 0, 880.0, &|| false).unwrap();
        assert_eq!(actual.rms_db, expected.rms_db);
        assert_eq!(actual.tone_seconds, expected.tone_seconds);
        assert_eq!(actual.tone_db, expected.tone_db);
        for hz in [0.0, -1.0, f64::NAN, f64::INFINITY, 24000.0] {
            assert!(analyze_file(&path, 0, hz, &|| false).is_err());
        }
        assert!(analyze_file(&path, 0, 880.0, &|| true).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
