//! Camera and connected device capture on Windows.

use anyhow::Result;
use crate::platform::AudioCaptureBackend;

#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
}

pub fn cameras() -> Vec<Device> {
    vec![]
}

pub fn microphones() -> Vec<Device> {
    crate::platform::windows::WindowsAudioBackend::new()
        .input_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| Device { id: d.id, name: d.name })
        .collect()
}

pub fn ios_devices() -> Vec<Device> {
    vec![]
}

pub fn enable_screen_capture_devices() {}

pub fn ios_dimensions(_id: &str) -> Option<(usize, usize)> {
    None
}

pub fn builtin_mic_off(_name: Option<&str>) -> bool {
    false
}

pub struct Camera;

impl Camera {
    pub fn start_with<F>(_sel: Option<&str>, _shared: F, _is_cam: bool) -> Result<Self> {
        Ok(Camera)
    }

    pub fn start_ios<F>(_id: &str, _shared: F) -> Result<Self> {
        Ok(Camera)
    }

    pub fn pause(&self, _now: f64) {}
    pub fn resume(&self, _now: f64) {}
    pub fn stop(&self) {}
}
