//! Voice-over capture in the editor: mono audio into the package while the preview plays.

use std::path::Path;
use std::time::Instant;
use anyhow::Result;

pub struct VoiceRecorder {
    started: Instant,
    pub file: String,
    pub start_source: f64,
}

impl VoiceRecorder {
    pub fn start(_package: &Path, file: &str, start_source: f64) -> Result<Self> {
        Ok(VoiceRecorder {
            started: Instant::now(),
            file: file.to_string(),
            start_source,
        })
    }

    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn stop(self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
}
