//! Bounded, random-access rendering of edited audio.
use crate::{
    engine::timemap::TimeMap,
    media::audio_file::{AudioCursor, AudioFile, Stereo},
    project::{
        model::Project,
        recording::{AudioTrack, Recording},
    },
};
use anyhow::{Result, ensure};
use std::path::Path;
pub const BLOCK_FRAMES: usize = 4096;
const RATE: f64 = 48000.0;

pub struct BlockMixer {
    map: TimeMap,
    frames: u64,
    system: Option<AudioCursor>,
    mic: Option<AudioCursor>,
    gains: Vec<(f32, f32)>,
    master: f32,
    voices: Vec<Voice>,
    duck: Option<f32>,
    music: Option<Music>,
    clicks: Option<Clicks>,
}
struct Voice {
    audio: AudioFile,
    cursor: Option<AudioCursor>,
    at: u64,
    gain: f32,
}
struct Music {
    cursor: AudioCursor,
    frames: u64,
    gain: f32,
}
struct Clicks {
    starts: Vec<u64>,
    samples: Vec<Stereo>,
    gain: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{
        model::{Slice, VoiceOver},
        recording::{MouseEvent, MouseKind, RecordingMeta},
    };
    fn fixture(label: &str) -> (std::path::PathBuf, Project, Recording) {
        let dir = std::env::temp_dir().join(format!(
            "cam-block-mix-{label}-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir_all(dir.join("recording")).unwrap();
        for (name, len, hz) in [
            ("system.wav", 96017, 0.04),
            ("mic.wav", 96017, 0.07),
            ("voice.wav", 30007, 0.085),
            ("music.wav", 17777, 0.015),
        ] {
            let samples: Vec<_> = (0..len)
                .map(|i| {
                    [
                        (i as f32 * hz).sin() * 0.3,
                        (i as f32 * hz * 0.8).cos() * 0.2,
                    ]
                })
                .collect();
            crate::app::player::write_wav(&dir.join("recording").join(name), &samples).unwrap();
        }
        let mut project = Project::default();
        project.style.audio.enhance_mic = false;
        project.style.audio.volume = 1.35;
        project.timeline.slices = vec![
            Slice {
                start: 23.333,
                end: 700.125,
                speed: 1.25,
                system_volume: 0.7,
                mic_volume: 0.3,
                ..Default::default()
            },
            Slice {
                start: 900.1,
                end: 1900.75,
                speed: 0.75,
                system_volume: 0.2,
                mic_volume: 0.8,
                ..Default::default()
            },
        ];
        let recording = Recording {
            meta: RecordingMeta {
                duration_ms: 2000.0,
                system_audio: Some(AudioTrack {
                    file: "system.wav".into(),
                    track: 0,
                }),
                mic: Some(AudioTrack {
                    file: "mic.wav".into(),
                    track: 0,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        (dir, project, recording)
    }
    fn compare(mixer: &mut BlockMixer, expected: &[Stereo], mastered: bool) {
        assert_eq!(mixer.frames(), expected.len() as u64);
        for (i, reference) in expected.chunks(4093).enumerate() {
            let mut actual = vec![[0.0; 2]; reference.len()];
            mixer
                .render((i * 4093) as u64, &mut actual, mastered, &|| false)
                .unwrap();
            let error = actual
                .iter()
                .zip(reference)
                .flat_map(|(a, b)| [(a[0] - b[0]).abs(), (a[1] - b[1]).abs()])
                .fold(0.0f32, f32::max);
            assert!(error < 2e-6, "block {i}, error {error}");
        }
        for start in [
            expected.len() as u64 - 20,
            4095,
            1,
            60000,
            100,
            expected.len() as u64 + 10,
        ] {
            let mut actual = [[0.0; 2]; 97];
            mixer
                .render(start, &mut actual, mastered, &|| false)
                .unwrap();
            for (i, a) in actual.iter().enumerate() {
                let e = expected
                    .get(start as usize + i)
                    .copied()
                    .unwrap_or([0.0; 2]);
                assert!(
                    (a[0] - e[0]).abs().max((a[1] - e[1]).abs()) < 2e-6,
                    "seek {start}, frame {i}"
                );
            }
        }
    }
    #[test]
    fn system_microphone_cuts_speed_gains_and_seeks_match_reference() {
        let (dir, project, recording) = fixture("base");
        let map = TimeMap::new(&project.timeline.slices, recording.meta.duration_ms);
        let mut mixer = BlockMixer::new(
            &project,
            &recording,
            Some(&dir),
            &dir.join("cache"),
            &map,
            &|| false,
        )
        .unwrap()
        .unwrap();
        compare(
            &mut mixer,
            &super::super::audio::mix_unmastered(&project, &recording, Some(&dir), &map).unwrap(),
            false,
        );
        compare(
            &mut mixer,
            &super::super::audio::mix(&project, &recording, Some(&dir), &map).unwrap(),
            true,
        );
    }
    #[test]
    fn all_effects_and_enhancement_match_reference_in_random_blocks() {
        for enhance in [false, true] {
            let (dir, mut project, mut recording) = fixture(&format!("effects-{enhance}"));
            project.style.audio.enhance_mic = enhance;
            project.style.audio.music = Some(
                dir.join("recording/music.wav")
                    .to_string_lossy()
                    .into_owned(),
            );
            project.style.audio.music_volume = 0.4;
            project.style.audio.click_sounds = true;
            project.style.audio.click_volume = 0.7;
            project.style.audio.duck_level = 0.2;
            project.timeline.voiceovers = vec![
                VoiceOver {
                    file: "voice.wav".into(),
                    start: 310.0,
                    volume: 0.6,
                    ..Default::default()
                },
                VoiceOver {
                    file: "voice.wav".into(),
                    start: 400.0,
                    volume: 0.8,
                    ..Default::default()
                },
                VoiceOver {
                    file: "voice.wav".into(),
                    start: 800.0,
                    volume: 0.5,
                    ..Default::default()
                },
            ];
            recording.mouse = [1000.0, 24.0, 700.0, 800.0, 300.0, 1890.0]
                .into_iter()
                .map(|t| MouseEvent {
                    t,
                    x: 0.0,
                    y: 0.0,
                    kind: MouseKind::Down,
                    button: 0,
                    cursor: 0,
                })
                .collect();
            let map = TimeMap::new(&project.timeline.slices, recording.meta.duration_ms);
            for mute_recorded in [false, true] {
                project.style.audio.mute_mic = mute_recorded;
                project.style.audio.mute_system = mute_recorded;
                let mut mixer = BlockMixer::new(
                    &project,
                    &recording,
                    Some(&dir),
                    &dir.join("cache"),
                    &map,
                    &|| false,
                )
                .unwrap()
                .unwrap();
                compare(
                    &mut mixer,
                    &super::super::audio::mix_unmastered(&project, &recording, Some(&dir), &map)
                        .unwrap(),
                    false,
                );
                compare(
                    &mut mixer,
                    &super::super::audio::mix(&project, &recording, Some(&dir), &map).unwrap(),
                    true,
                );
            }
        }
    }
    #[test]
    fn missing_sources_cancellation_and_render_bounds_are_explicit() {
        let (dir, mut project, recording) = fixture("bounds");
        let map = TimeMap::new(&project.timeline.slices, recording.meta.duration_ms);
        let mut mixer = BlockMixer::new(
            &project,
            &recording,
            Some(&dir),
            &dir.join("cache"),
            &map,
            &|| false,
        )
        .unwrap()
        .unwrap();
        assert!(
            mixer
                .render(0, &mut vec![[0.0; 2]; BLOCK_FRAMES + 1], false, &|| false)
                .is_err()
        );
        assert!(
            mixer
                .render(0, &mut [[0.0; 2]; 10], false, &|| true)
                .is_err()
        );
        assert!(
            BlockMixer::new(
                &project,
                &recording,
                Some(&dir),
                &dir.join("cache"),
                &map,
                &|| true
            )
            .is_err()
        );
        project.style.audio.mute_mic = true;
        project.style.audio.mute_system = true;
        assert!(
            BlockMixer::new(
                &project,
                &recording,
                Some(&dir),
                &dir.join("cache"),
                &map,
                &|| false
            )
            .unwrap()
            .is_none()
        );
        project.style.audio.music = Some(dir.join("missing.wav").to_string_lossy().into_owned());
        assert!(
            BlockMixer::new(
                &project,
                &recording,
                Some(&dir),
                &dir.join("cache"),
                &map,
                &|| false
            )
            .is_err()
        );
    }
}
fn track(
    path: &Path,
    index: usize,
    cache: &Path,
    enhance: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<AudioFile> {
    let asset = AudioFile::decode_cached(path, index, cache, cancelled)?;
    if enhance {
        super::voice_cache::clean_cached(&asset, cancelled)
    } else {
        Ok(asset)
    }
}
fn optional_track(
    dir: Option<&Path>,
    info: &Option<AudioTrack>,
    cache: &Path,
    muted: bool,
    enhance: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<AudioCursor>> {
    if muted {
        return Ok(None);
    }
    match (dir, info) {
        (Some(dir), Some(info)) => Ok(Some(
            track(
                &dir.join("recording").join(&info.file),
                info.track,
                cache,
                enhance,
                cancelled,
            )?
            .cursor()?,
        )),
        _ => Ok(None),
    }
}
impl BlockMixer {
    pub fn new(
        project: &Project,
        recording: &Recording,
        dir: Option<&Path>,
        cache: &Path,
        map: &TimeMap,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Self>> {
        ensure!(!cancelled(), "audio mixing cancelled");
        let count = (map.duration() / 1000.0 * RATE).ceil();
        ensure!(
            count.is_finite() && count >= 0.0 && count < (u64::MAX / 8) as f64,
            "invalid audio timeline duration"
        );
        let style = &project.style.audio;
        let system = optional_track(
            dir,
            &recording.meta.system_audio,
            cache,
            style.mute_system,
            false,
            cancelled,
        )?;
        let mic = optional_track(
            dir,
            &recording.meta.mic,
            cache,
            style.mute_mic,
            style.enhance_mic,
            cancelled,
        )?;
        let mut voices = Vec::new();
        if let Some(dir) = dir {
            for voice in &project.timeline.voiceovers {
                let audio = track(
                    &dir.join("recording").join(&voice.file),
                    0,
                    cache,
                    style.enhance_mic,
                    cancelled,
                )?;
                voices.push(Voice {
                    audio,
                    cursor: None,
                    at: (map.to_playback_nearest(voice.start) / 1000.0 * RATE) as u64,
                    gain: voice.volume as f32,
                });
            }
        }
        let music = if let Some(path) = &style.music {
            let asset = track(Path::new(path), 0, cache, false, cancelled)?;
            Some(Music {
                cursor: asset.cursor()?,
                frames: asset.frames(),
                gain: style.music_volume as f32,
            })
        } else {
            None
        };
        let clicks = style.click_sounds.then(|| {
            let mut starts: Vec<_> = recording
                .mouse
                .iter()
                .filter(|e| e.kind == crate::project::recording::MouseKind::Down)
                .filter_map(|e| map.to_playback(e.t))
                .map(|t| (t / 1000.0 * RATE) as u64)
                .collect();
            starts.sort_unstable();
            Clicks {
                starts,
                samples: super::audio::click_sound(),
                gain: style.click_volume as f32,
            }
        });
        if system.is_none()
            && mic.is_none()
            && voices.is_empty()
            && music.is_none()
            && clicks.is_none()
        {
            return Ok(None);
        }
        Ok(Some(Self {
            map: map.clone(),
            frames: count as u64,
            system,
            mic,
            gains: project
                .timeline
                .slices
                .iter()
                .map(|s| (s.system_volume as f32, s.mic_volume as f32))
                .collect(),
            master: style.volume as f32,
            voices,
            music,
            clicks,
            duck: style
                .duck_under_voiceover
                .then_some(style.duck_level as f32),
        }))
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn render(
        &mut self,
        start: u64,
        output: &mut [Stereo],
        mastered: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        ensure!(
            output.len() <= BLOCK_FRAMES,
            "audio render block exceeds {BLOCK_FRAMES} frames"
        );
        ensure!(!cancelled(), "audio mixing cancelled");
        output.fill([0.0; 2]);
        let end = start.saturating_add(output.len() as u64).min(self.frames);
        for link in &self.map.links {
            let lo = ((link.pb_start / 1000.0 * RATE) as u64).max(start);
            let hi = ((link.pb_end / 1000.0 * RATE) as u64).min(end);
            let (system_gain, mic_gain) = self.gains.get(link.slice).copied().unwrap_or((1.0, 1.0));
            for (cursor, gain) in [(&mut self.system, system_gain), (&mut self.mic, mic_gain)] {
                if gain <= 0.0 {
                    continue;
                }
                if let Some(cursor) = cursor {
                    for j in lo..hi {
                        let playback = j as f64 / RATE * 1000.0;
                        let source = (link.src_start + (playback - link.pb_start) * link.speed)
                            / 1000.0
                            * RATE;
                        let sample = cursor.sample_at(source)?;
                        let out = &mut output[(j - start) as usize];
                        out[0] += sample[0] * gain;
                        out[1] += sample[1] * gain;
                    }
                }
            }
        }
        if let Some(level) = self.duck {
            let mut envelope = vec![1.0f32; output.len()];
            const RAMP: u64 = 7200;
            for voice in &self.voices {
                let a = voice.at.min(self.frames);
                let b = voice
                    .at
                    .saturating_add(voice.audio.frames())
                    .min(self.frames);
                for j in a.saturating_sub(RAMP).max(start)..b.saturating_add(RAMP).min(end) {
                    let t = if j < a {
                        (a - j) as f32 / RAMP as f32
                    } else if j >= b {
                        (j - b) as f32 / RAMP as f32
                    } else {
                        0.0
                    };
                    let g = level + (1.0 - level) * t.clamp(0.0, 1.0);
                    let e = &mut envelope[(j - start) as usize];
                    *e = e.min(g);
                }
            }
            for (s, g) in output.iter_mut().zip(envelope) {
                s[0] *= g;
                s[1] *= g;
            }
        }
        let mut narration = vec![[0.0; 2]; output.len()];
        let mut voice_samples = vec![[0.0; 2]; output.len()];
        for voice in &mut self.voices {
            let lo = voice.at.max(start);
            let hi = voice.at.saturating_add(voice.audio.frames()).min(end);
            if lo >= hi {
                voice.cursor = None;
                continue;
            }
            if voice.cursor.is_none() {
                voice.cursor = Some(voice.audio.cursor()?);
            }
            let count = (hi - lo) as usize;
            voice
                .cursor
                .as_mut()
                .unwrap()
                .read_frames(lo - voice.at, &mut voice_samples[..count])?;
            for (i, s) in voice_samples[..count].iter().enumerate() {
                let out = &mut narration[(lo - start) as usize + i];
                out[0] += s[0] * voice.gain;
                out[1] += s[1] * voice.gain;
            }
        }
        for (out, voice) in output.iter_mut().zip(narration) {
            out[0] += voice[0];
            out[1] += voice[1];
        }
        if let Some(music) = &mut self.music {
            let fade = self.frames.min(RATE as u64).max(1);
            for j in start..end {
                let f = if j.saturating_add(fade) > self.frames {
                    (self.frames - j) as f32 / fade as f32
                } else {
                    1.0
                };
                let sample = music.cursor.sample_at((j % music.frames) as f64)?;
                let out = &mut output[(j - start) as usize];
                out[0] += sample[0] * music.gain * f;
                out[1] += sample[1] * music.gain * f;
            }
        }
        if let Some(clicks) = &self.clicks {
            let first = clicks
                .starts
                .partition_point(|at| at.saturating_add(clicks.samples.len() as u64) <= start);
            let last = clicks.starts.partition_point(|at| *at < end).max(first);
            for &at in &clicks.starts[first..last] {
                for j in at.max(start)..at.saturating_add(clicks.samples.len() as u64).min(end) {
                    let sample = clicks.samples[(j - at) as usize];
                    let out = &mut output[(j - start) as usize];
                    out[0] += sample[0] * clicks.gain;
                    out[1] += sample[1] * clicks.gain;
                }
            }
        }
        if mastered {
            for sample in output {
                sample[0] = super::audio::soft_clip(sample[0] * self.master);
                sample[1] = super::audio::soft_clip(sample[1] * self.master);
            }
        }
        Ok(())
    }
}
