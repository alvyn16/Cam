//! Microphone setup meter, sharing capture's device lifecycle.
use crate::platform::{AudioCaptureBackend, audio::AudioStreamSession};
use anyhow::Result;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

pub struct MicMeter {
    level: Arc<AtomicU32>,
    session: Box<dyn AudioStreamSession>,
}
impl MicMeter {
    /// Meter the requested input, or the default input when none is selected.
    pub fn start(name: Option<&str>) -> Result<Self> {
        let level = Arc::new(AtomicU32::new(0));
        let output = level.clone();
        let session = crate::platform::windows::WindowsAudioBackend::new().start_microphone(
            name,
            48000,
            2,
            Box::new(move |samples, _| {
                let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                output.fetch_max(peak.to_bits(), Ordering::Relaxed);
            }),
        )?;
        Ok(Self { level, session })
    }
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }
}
impl Drop for MicMeter {
    fn drop(&mut self) {
        self.session.stop();
    }
}
