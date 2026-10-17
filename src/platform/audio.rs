//! Audio capture and loopback abstractions.

use anyhow::Result;

#[derive(Clone, Debug)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub trait AudioStreamSession: Send {
    fn stop(&mut self);
}

pub trait AudioCaptureBackend: Send + Sync {
    fn input_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
    fn start_microphone(
        &self,
        device_id: Option<&str>,
        sample_rate: u32,
        channels: u16,
        on_samples: Box<dyn FnMut(&[f32], f64) + Send + 'static>,
    ) -> Result<Box<dyn AudioStreamSession>>;
    fn start_system_loopback(
        &self,
        sample_rate: u32,
        channels: u16,
        on_samples: Box<dyn FnMut(&[f32], f64) + Send + 'static>,
    ) -> Result<Box<dyn AudioStreamSession>>;
}
