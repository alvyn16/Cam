//! Disk-backed voice enhancement, with state retained across all input blocks.
use crate::media::audio_file::{AudioFile, AudioFileWriter, Stereo};
use anyhow::{Result, ensure};
use std::path::Path;

struct TemporaryAudio(AudioFile);
impl Drop for TemporaryAudio {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.path());
    }
}
pub fn clean_cached(source: &AudioFile, cancelled: &dyn Fn() -> bool) -> Result<AudioFile> {
    ensure!(!cancelled(), "voice enhancement cancelled");
    let key = source.identity()?;
    let parent = source.path().parent().expect("audio asset has a parent");
    let destination = parent.join(format!("voice-v3-{key:016x}.pcm"));
    if let Ok(asset) = AudioFile::open_pcm(&destination) {
        if asset.frames() == source.frames() {
            return Ok(asset);
        }
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let work = parent.join(format!(
        "voice-v3-{key:016x}.{}.{}.{}.work.pcm",
        std::process::id(),
        crate::util::host_now().to_bits(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let (denoised, voiced) = denoise(source, &work, cancelled)?;
    let denoised = TemporaryAudio(denoised);
    ensure!(
        source.identity()? == key,
        "voice source changed during enhancement"
    );
    level(&denoised.0, &destination, voiced.gain(), cancelled)
}

const N: usize = nnnoiseless::DenoiseState::FRAME_SIZE;
const FLOOR: f32 = 0.006;
#[derive(Default)]
struct Voiced {
    energy: f64,
    frames: u64,
}
impl Voiced {
    fn add(&mut self, samples: &[Stereo]) {
        for s in samples {
            let energy = (s[0] * s[0] + s[1] * s[1]) / 2.0;
            if energy > FLOOR * FLOOR {
                self.energy += energy as f64;
                self.frames += 1;
            }
        }
    }
    fn gain(&self) -> f32 {
        if self.frames == 0 {
            1.0
        } else {
            (0.125 / (self.energy / self.frames as f64).sqrt().max(1e-6)).min(4.0) as f32
        }
    }
}
const AGC_WINDOW: usize = 2400;
struct Agc {
    gain: f32,
    level: f32,
}
impl Agc {
    fn next(&mut self, samples: &[Stereo]) -> f32 {
        let rms = (samples
            .iter()
            .map(|s| (s[0] * s[0] + s[1] * s[1]) * 0.5)
            .sum::<f32>()
            / samples.len() as f32)
            .sqrt();
        if rms > FLOOR {
            self.level += (rms - self.level) * 0.1;
            let want = (0.125 / self.level.max(1e-6)).clamp(0.35, 2.8);
            self.gain += (want - self.gain) * if want < self.gain { 0.15 } else { 0.05 };
        }
        self.gain
    }
}
fn scaled(
    cursor: &mut crate::media::audio_file::AudioCursor,
    at: u64,
    buffer: &mut [Stereo],
    gain: f32,
) -> Result<usize> {
    let count = cursor.read_frames(at, buffer)?;
    for s in &mut buffer[..count] {
        s[0] *= gain;
        s[1] *= gain;
    }
    Ok(count)
}
fn level(
    source: &AudioFile,
    destination: &Path,
    gain: f32,
    cancelled: &dyn Fn() -> bool,
) -> Result<AudioFile> {
    let mut cursor = source.cursor()?;
    let mut writer = AudioFileWriter::create(destination)?;
    let (mut current, mut next) = (vec![[0.0; 2]; AGC_WINDOW], vec![[0.0; 2]; AGC_WINDOW]);
    let mut agc = Agc {
        gain: 1.0,
        level: 0.125,
    };
    let mut count = scaled(&mut cursor, 0, &mut current, gain)?;
    let mut next_count = scaled(&mut cursor, AGC_WINDOW as u64, &mut next, gain)?;
    let bypass = source.frames() < AGC_WINDOW as u64;
    let mut current_gain = if bypass {
        1.0
    } else {
        agc.next(&current[..count])
    };
    let mut next_gain = if next_count == 0 {
        current_gain
    } else {
        agc.next(&next[..next_count])
    };
    let mut previous_gain = current_gain;
    let windows = source.frames().div_ceil(AGC_WINDOW as u64);
    let mut at = 0;
    while count > 0 {
        ensure!(!cancelled(), "voice enhancement cancelled");
        let window = at / AGC_WINDOW as u64;
        for (i, s) in current[..count].iter_mut().enumerate() {
            let x = (at + i as u64) as f32 / AGC_WINDOW as f32 - 0.5;
            let j = (x.max(0.0) as u64).min(windows - 1);
            let f = (x - j as f32).clamp(0.0, 1.0);
            let (a, b) = if j < window {
                (previous_gain, current_gain)
            } else {
                (current_gain, next_gain)
            };
            let g = a + (b - a) * f;
            s[0] *= g;
            s[1] *= g;
        }
        writer.write_frames(&current[..count])?;
        at += count as u64;
        std::mem::swap(&mut current, &mut next);
        count = next_count;
        previous_gain = current_gain;
        current_gain = next_gain;
        next_count = scaled(&mut cursor, at + AGC_WINDOW as u64, &mut next, gain)?;
        next_gain = if next_count == 0 {
            current_gain
        } else {
            agc.next(&next[..next_count])
        };
    }
    ensure!(!cancelled(), "voice enhancement cancelled");
    writer.finish()
}
fn denoise(
    source: &AudioFile,
    destination: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<(AudioFile, Voiced)> {
    let mut input = source.cursor()?;
    let mut writer = AudioFileWriter::create(destination)?;
    let mut state = nnnoiseless::DenoiseState::new();
    let mut stereo = [[0.0; 2]; N];
    let mut mono = [0.0; N];
    let mut output = [0.0; N];
    let mut result = [[0.0; 2]; N];
    let mut voiced = Voiced::default();
    for block in 0..=source.frames().div_ceil(N as u64) {
        ensure!(!cancelled(), "voice enhancement cancelled");
        input.read_frames(block * N as u64, &mut stereo)?;
        for (dst, s) in mono.iter_mut().zip(stereo) {
            *dst = (s[0] + s[1]) * 0.5 * 32768.0;
        }
        state.process_frame(&mut output, &mono);
        if block > 0 {
            for (dst, s) in result.iter_mut().zip(output) {
                *dst = [(s / 32768.0).clamp(-1.0, 1.0); 2];
            }
            let count = source
                .frames()
                .saturating_sub((block - 1) * N as u64)
                .min(N as u64) as usize;
            voiced.add(&result[..count]);
            writer.write_frames(&result[..count])?;
        }
    }
    ensure!(!cancelled(), "voice enhancement cancelled");
    Ok((writer.finish()?, voiced))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(samples: &[Stereo], label: &str) -> AudioFile {
        let dir = std::env::temp_dir().join(format!(
            "cam-voice-cache-{label}-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&dir).unwrap();
        let mut writer = AudioFileWriter::create(&dir.join("raw.pcm")).unwrap();
        writer.write_frames(samples).unwrap();
        writer.finish().unwrap()
    }
    #[test]
    fn streamed_denoising_matches_reference_including_latency_and_partial_tail() {
        let samples: Vec<_> = (0..12345)
            .map(|i| {
                let v = (i as f32 * 0.071).sin() * 0.06;
                [v, v * 0.5]
            })
            .collect();
        let raw = source(&samples, "denoise");
        let destination = raw.path().with_file_name("denoised.pcm");
        let (asset, voiced) = denoise(&raw, &destination, &|| false).unwrap();
        let expected = crate::export::isolate::isolate_voice(&samples).unwrap();
        let mut actual = vec![[0.0; 2]; samples.len()];
        asset.cursor().unwrap().read_frames(0, &mut actual).unwrap();
        assert_eq!(actual, expected);
        assert!(voiced.gain().is_finite());
    }
    #[test]
    fn streamed_enhancement_matches_buffered_reference_across_agc_boundaries() {
        for len in [1, 479, 480, 481, 2399, 2400, 2401, 48001, 96013] {
            let samples: Vec<_> = (0..len)
                .map(|i| {
                    let amplitude = if i % 24000 < 12000 { 0.6 } else { 0.06 };
                    let value = (i as f32 * 0.063).sin() * amplitude;
                    [value, value * 0.4 + (i as f32 * 0.17).sin() * 0.01]
                })
                .collect();
            let raw = source(&samples, &format!("level-{len}"));
            let result = clean_cached(&raw, &|| false).unwrap();
            let mut expected = samples;
            crate::export::audio::clean_voice(&mut expected);
            let mut actual = vec![[0.0; 2]; len];
            result
                .cursor()
                .unwrap()
                .read_frames(0, &mut actual)
                .unwrap();
            let error = actual
                .iter()
                .zip(&expected)
                .flat_map(|(a, b)| [(a[0] - b[0]).abs(), (a[1] - b[1]).abs()])
                .fold(0.0f32, f32::max);
            assert!(error < 2e-6, "length {len}, maximum error {error}");
            assert_eq!(result.frames(), len as u64);
            let modified = result.path().metadata().unwrap().modified().unwrap();
            let again = clean_cached(&raw, &|| false).unwrap();
            assert_eq!(again.path(), result.path());
            assert_eq!(
                again.path().metadata().unwrap().modified().unwrap(),
                modified
            );
            assert_eq!(
                std::fs::read_dir(raw.path().parent().unwrap())
                    .unwrap()
                    .filter(|e| e
                        .as_ref()
                        .unwrap()
                        .path()
                        .extension()
                        .is_some_and(|e| e == "pcm"))
                    .count(),
                2
            );
        }
    }
    #[test]
    fn cancellation_in_both_dsp_passes_removes_intermediate_files() {
        for during_level in [false, true] {
            let raw = source(&vec![[0.2, -0.1]; 9600], &format!("cancel-{during_level}"));
            let parent = raw.path().parent().unwrap();
            let polls = std::cell::Cell::new(0);
            let level_seen = std::cell::Cell::new(false);
            let result = clean_cached(&raw, &|| {
                polls.set(polls.get() + 1);
                if during_level {
                    let has_work = std::fs::read_dir(parent).unwrap().any(|e| {
                        e.unwrap()
                            .file_name()
                            .to_string_lossy()
                            .ends_with(".work.pcm")
                    });
                    level_seen.set(has_work);
                    has_work
                } else {
                    polls.get() >= 3
                }
            });
            assert!(result.unwrap_err().to_string().contains("cancelled"));
            assert_eq!(level_seen.get(), during_level);
            assert_eq!(std::fs::read_dir(parent).unwrap().count(), 1);
            assert!(raw.path().exists());
        }
    }
    #[test]
    fn silence_source_invalidation_and_concurrent_jobs_keep_complete_caches() {
        let raw = source(&[[0.0; 2]; 2401], "identity");
        let silent = clean_cached(&raw, &|| false).unwrap();
        assert_eq!(silent.peak(&|| false).unwrap(), 0.0);
        assert!(clean_cached(&raw, &|| true).is_err());
        let mut writer = AudioFileWriter::create(raw.path()).unwrap();
        writer.write_frames(&[[0.2; 2]; 9600]).unwrap();
        let raw = writer.finish().unwrap();
        let a = raw.clone();
        let b = raw.clone();
        let a = std::thread::spawn(move || clean_cached(&a, &|| false).unwrap());
        let b = std::thread::spawn(move || clean_cached(&b, &|| false).unwrap());
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        assert_eq!(a.path(), b.path());
        assert_ne!(a.path(), silent.path());
        assert_eq!(a.frames(), 9600);
        assert_eq!(
            std::fs::read_dir(raw.path().parent().unwrap())
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|e| e == "pcm"))
                .count(),
            3
        );
    }
    #[test]
    fn failed_enhancement_publication_keeps_sources_and_removes_work_files() {
        let raw = source(&[[0.1; 2]; 2401], "blocked");
        let parent = raw.path().parent().unwrap();
        let blocked = parent.join(format!("voice-v3-{:016x}.pcm", raw.identity().unwrap()));
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("keep"), b"keep").unwrap();
        assert!(clean_cached(&raw, &|| false).is_err());
        assert_eq!(std::fs::read(blocked.join("keep")).unwrap(), b"keep");
        assert!(raw.path().exists());
        assert_eq!(std::fs::read_dir(parent).unwrap().count(), 2);
    }
}
