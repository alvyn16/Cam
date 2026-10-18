//! Voice-over capture in the editor: mono audio into the package while the preview plays.

use anyhow::Result;
use std::path::Path;
use std::time::Instant;

pub struct VoiceRecorder {
    stem: Option<crate::capture::audio::Stem>,
    started: Instant,
    pub file: String,
    pub start_source: f64,
}

impl VoiceRecorder {
    pub fn start(package: &Path, file: &str, start_source: f64) -> Result<Self> {
        let clock = std::sync::Arc::new(std::sync::Mutex::new(crate::capture::audio::Clock {
            start: Some(crate::util::host_now()),
            ..Default::default()
        }));
        let stem = crate::capture::audio::Stem::start(
            crate::capture::audio::Source::Microphone(None),
            clock,
            &package.join("recording").join(file),
        )?;
        Ok(VoiceRecorder {
            stem: Some(stem),
            started: Instant::now(),
            file: file.to_string(),
            start_source,
        })
    }

    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn stop(mut self) -> Result<f64> {
        let seconds = self.started.elapsed().as_secs_f64();
        if let Some(stem) = self.stem.take() {
            stem.finish(seconds)?;
        }
        Ok(seconds * 1000.0)
    }
}
