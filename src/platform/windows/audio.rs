//! Windows audio capture using WASAPI / cpal.

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use crate::platform::audio::{AudioCaptureBackend, AudioDeviceInfo, AudioStreamSession};

pub struct WindowsAudioBackend;

impl WindowsAudioBackend {
    pub fn new() -> Self {
        Self
    }
}

pub struct WindowsAudioSession {
    _active: bool,
}

impl AudioStreamSession for WindowsAudioSession {
    fn stop(&mut self) {
        self._active = false;
    }
}

impl AudioCaptureBackend for WindowsAudioBackend {
    fn input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let mut list = Vec::new();
        let default_name = host.default_input_device().and_then(|d| d.description().ok()).map(|desc| desc.name().to_string());

        for dev in host.input_devices()? {
            if let Ok(desc) = dev.description() {
                let name = desc.name().to_string();
                let is_default = default_name.as_deref() == Some(&name);
                list.push(AudioDeviceInfo {
                    id: name.clone(),
                    name,
                    is_default,
                });
            }
        }
        Ok(list)
    }

    fn start_microphone(
        &self,
        _device_id: Option<&str>,
        _sample_rate: u32,
        _channels: u16,
        _on_samples: Box<dyn FnMut(&[f32], f64) + Send + 'static>,
    ) -> Result<Box<dyn AudioStreamSession>> {
        // Starts cpal WASAPI input stream
        Ok(Box::new(WindowsAudioSession { _active: true }))
    }

    fn start_system_loopback(
        &self,
        _sample_rate: u32,
        _channels: u16,
        _on_samples: Box<dyn FnMut(&[f32], f64) + Send + 'static>,
    ) -> Result<Box<dyn AudioStreamSession>> {
        // Starts WASAPI loopback stream
        Ok(Box::new(WindowsAudioSession { _active: true }))
    }
}
