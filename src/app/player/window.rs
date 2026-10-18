//! Bounded preview prefetch. Only this worker reads audio files; the device
//! callback samples immutable in-memory windows.
use super::{Shared, Stereo, sample};
use crate::export::block_mix::{BLOCK_FRAMES, BlockMixer};
use anyhow::Result;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

const LOOKAHEAD: usize = BLOCK_FRAMES * 4;

pub(super) struct Window {
    start: u64,
    total: u64,
    samples: Vec<Stereo>,
}
impl Window {
    pub(super) fn sample(&self, position: f64) -> Stereo {
        let relative = position - self.start as f64;
        if !relative.is_finite() || relative < 0.0 {
            return [0.0; 2];
        }
        if position as u64 == self.total.saturating_sub(1) {
            return self
                .samples
                .get(relative as usize)
                .copied()
                .unwrap_or([0.0; 2]);
        }
        sample(&self.samples, relative)
    }

    fn covers(&self, position: u64) -> bool {
        position >= self.start
            && position.saturating_add(BLOCK_FRAMES as u64)
                < self.start.saturating_add(self.samples.len() as u64)
    }

    fn render(mixer: &mut BlockMixer, start: u64, cancelled: &dyn Fn() -> bool) -> Result<Self> {
        // Retain the final interpolation sample, including a zero beyond EOF.
        let mut samples = vec![[0.0; 2]; LOOKAHEAD + 1];
        for (block, output) in samples.chunks_mut(BLOCK_FRAMES).enumerate() {
            mixer.render(
                start.saturating_add((block * BLOCK_FRAMES) as u64),
                output,
                false,
                cancelled,
            )?;
        }
        Ok(Self {
            start,
            total: mixer.frames(),
            samples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::player::AudioPreview,
        engine::timemap::TimeMap,
        project::{
            model::Project,
            recording::{AudioTrack, Recording, RecordingMeta},
        },
    };
    use std::{path::PathBuf, time::Instant};

    fn fixture() -> (PathBuf, Project, Recording) {
        let dir = std::env::temp_dir().join(format!(
            "cam-preview-window-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir_all(dir.join("recording")).unwrap();
        let samples: Vec<_> = (0..96000)
            .map(|i| [(i as f32 * 0.1).sin() * 0.2, (i as f32 * 0.2).cos() * 0.15])
            .collect();
        crate::app::player::write_wav(&dir.join("recording/source.wav"), &samples).unwrap();
        let mut project = Project::default();
        project.style.audio.enhance_mic = false;
        let recording = Recording {
            meta: RecordingMeta {
                duration_ms: 2000.0,
                system_audio: Some(AudioTrack {
                    file: "source.wav".into(),
                    track: 0,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        (dir, project, recording)
    }
    fn wait(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !predicate() {
            assert!(Instant::now() < deadline, "preview worker timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn windows_match_reference_across_seeks_boundaries_and_eof() {
        let (dir, project, recording) = fixture();
        let map = TimeMap::new(&project.timeline.slices, recording.meta.duration_ms);
        let reference =
            crate::export::audio::mix_unmastered(&project, &recording, Some(&dir), &map).unwrap();
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
        for start in [0, 12345, 95000, 100, 96000, 200000] {
            let window = Window::render(&mut mixer, start, &|| false).unwrap();
            assert_eq!(window.samples.len(), LOOKAHEAD + 1);
            for offset in [0.0, 0.5, 4095.9, 8191.25, 999.75, 16383.0] {
                let position = start as f64 + offset;
                let actual = window.sample(position);
                let expected = sample(&reference, position);
                assert!(
                    (actual[0] - expected[0]).abs() < 2e-6
                        && (actual[1] - expected[1]).abs() < 2e-6,
                    "at {position}: {actual:?} != {expected:?}"
                );
            }
            assert_eq!(window.sample(start as f64 - 1.0), [0.0; 2]);
        }
    }

    #[test]
    fn producer_replaces_seeks_reports_errors_and_joins() {
        let (dir, mut project, recording) = fixture();
        let mut preview = AudioPreview::new();
        preview.rebuild_now(&project, &recording, &dir);
        wait(|| {
            preview.tick();
            !preview.mixing()
        });
        assert!(preview.has_audio());
        for position in [1800.0, 10.0, 900.0] {
            preview.seek(position);
            wait(|| {
                preview
                    .shared
                    .mix
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|window| window.covers((position * 48.0) as u64))
            });
        }
        // Several superseded jobs must not publish over the last mute.
        for mute in [true, false, true, false, true] {
            project.style.audio.mute_system = mute;
            preview.rebuild_now(&project, &recording, &dir);
        }
        wait(|| {
            preview.tick();
            !preview.mixing()
        });
        assert!(!preview.has_audio());
        project.style.audio.mute_system = false;
        let mut missing = recording.clone();
        missing.meta.system_audio.as_mut().unwrap().file = "missing.wav".into();
        preview.rebuild_now(&project, &missing, &dir);
        wait(|| {
            preview.tick();
            !preview.mixing()
        });
        assert!(!preview.has_audio());
        assert!(
            preview
                .error
                .as_ref()
                .is_some_and(|e| e.contains("missing.wav")),
            "{:?}",
            preview.error
        );
        preview.rebuild_now(&project, &recording, &dir);
        preview.shutdown();
        assert!(preview.producer.is_none());
        assert!(preview.preparations.is_empty());
        preview.shutdown();
    }
}

pub(super) struct Prepared {
    pub generation: u64,
    pub result: Result<Option<BlockMixer>, String>,
}

fn publish(shared: &Shared, window: Option<Arc<Window>>) {
    *shared.mix.lock().unwrap_or_else(|e| e.into_inner()) = window;
    shared.mix_gen.fetch_add(1, Ordering::Release);
}

pub(super) fn produce(shared: Arc<Shared>) {
    let mut mixer: Option<BlockMixer> = None;
    let mut current: Option<Arc<Window>> = None;
    let stopped = || shared.stopped.load(Ordering::Acquire);
    while !stopped() {
        let prepared = shared
            .prepared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let mut adopted = None;
        if let Some(Prepared { generation, result }) = prepared {
            if generation == shared.requested.load(Ordering::Acquire) {
                let (next, error) = match result {
                    Ok(next) => (next, None),
                    Err(error) => (None, Some(error)),
                };
                mixer = next;
                current = None;
                *shared.mix_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
                adopted = Some(generation);
            }
        }
        let position = f64::from_bits(shared.pos.load(Ordering::Relaxed)).max(0.0) as u64;
        if let Some(graph) = &mut mixer {
            if current
                .as_ref()
                .is_none_or(|window| !window.covers(position))
            {
                match Window::render(graph, position, &stopped) {
                    Ok(window) => {
                        let window = Arc::new(window);
                        publish(&shared, Some(window.clone()));
                        current = Some(window);
                    }
                    Err(error) => {
                        *shared.mix_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(Some(format!("Couldn't render preview audio: {error:#}")));
                        mixer = None;
                        current = None;
                        publish(&shared, None);
                    }
                }
            }
        } else if adopted.is_some() {
            publish(&shared, None);
        }
        if let Some(generation) = adopted {
            // Settled means the first window (or explicit silence/error) is published.
            shared.applied.store(generation, Ordering::Release);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
