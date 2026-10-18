//! Windows WASAPI capture. cpal enables loopback on an output device.
mod process;
use crate::platform::audio::{AudioCaptureBackend, AudioDeviceInfo, AudioStreamSession};
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
pub struct WindowsAudioBackend;
impl WindowsAudioBackend {
    pub fn new() -> Self {
        Self
    }
    /// Capture 48 kHz stereo system audio, excluding this process and its children.
    pub fn start_loopback_excluding_self(
        &self,
        on_samples: Box<dyn FnMut(&[f32], f64) + Send + 'static>,
    ) -> Result<Box<dyn AudioStreamSession>> {
        process::start(on_samples)
    }
}
pub struct WindowsAudioSession {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
}
impl AudioStreamSession for WindowsAudioSession {
    fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
impl Drop for WindowsAudioSession {
    fn drop(&mut self) {
        self.stop();
    }
}
impl AudioCaptureBackend for WindowsAudioBackend {
    fn input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let default = host
            .default_input_device()
            .and_then(|d| d.description().ok())
            .map(|d| d.name().to_owned());
        Ok(host
            .input_devices()?
            .filter_map(|d| d.description().ok())
            .map(|d| {
                let name = d.name().to_owned();
                AudioDeviceInfo {
                    id: name.clone(),
                    is_default: default.as_ref() == Some(&name),
                    name,
                }
            })
            .collect())
    }
    fn start_microphone(
        &self,
        id: Option<&str>,
        rate: u32,
        channels: u16,
        on_samples: Box<dyn FnMut(&[f32], f64) + Send + 'static>,
    ) -> Result<Box<dyn AudioStreamSession>> {
        start(id.map(str::to_owned), false, rate, channels, on_samples)
    }
    fn start_system_loopback(
        &self,
        rate: u32,
        channels: u16,
        on_samples: Box<dyn FnMut(&[f32], f64) + Send + 'static>,
    ) -> Result<Box<dyn AudioStreamSession>> {
        start(None, true, rate, channels, on_samples)
    }
}
fn start(
    id: Option<String>,
    loopback: bool,
    rate: u32,
    channels: u16,
    mut callback: Box<dyn FnMut(&[f32], f64) + Send>,
) -> Result<Box<dyn AudioStreamSession>> {
    anyhow::ensure!(
        rate == 48000 && channels == 2,
        "capture currently requires 48 kHz stereo"
    );
    let stop = Arc::new(AtomicBool::new(false));
    let end = stop.clone();
    let error = Arc::new(Mutex::new(None));
    let errors = error.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let open = || -> Result<cpal::Stream> {
            let host = cpal::default_host();
            let device = if loopback {
                host.default_output_device()
            } else if let Some(id) = id {
                host.input_devices()?
                    .find(|d| d.description().is_ok_and(|n| n.name() == id))
            } else {
                host.default_input_device()
            }
            .context("requested audio device unavailable")?;
            let supported = if loopback {
                device.default_output_config()?
            } else {
                device.default_input_config()?
            };
            let config = supported.config();
            let n = config.channels as usize;
            let source_rate = config.sample_rate as f64;
            let mut receive = move |data: Vec<f32>| {
                let stereo: Vec<_> = data
                    .chunks(n)
                    .map(|c| [c[0], *c.get(1).unwrap_or(&c[0])])
                    .collect();
                let stereo = crate::capture::mixer::resample(stereo, source_rate);
                let flat: Vec<_> = stereo.iter().flat_map(|s| s.iter().copied()).collect();
                callback(
                    &flat,
                    crate::util::host_now() - stereo.len() as f64 / 48000.0,
                );
            };
            let err = move |e: cpal::Error| {
                eprintln!("Cam audio stream: {e}");
                if !matches!(
                    e.kind(),
                    cpal::ErrorKind::Xrun
                        | cpal::ErrorKind::DeviceChanged
                        | cpal::ErrorKind::RealtimeDenied
                ) {
                    *errors.lock().unwrap() = Some(format!("Audio capture device failed: {e}"));
                }
            };
            let stream = match supported.sample_format() {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    config,
                    move |d: &[f32], _: &_| receive(d.to_vec()),
                    err,
                    None,
                )?,
                cpal::SampleFormat::I16 => device.build_input_stream(
                    config,
                    move |d: &[i16], _: &_| {
                        receive(d.iter().map(|s| *s as f32 / 32768.0).collect())
                    },
                    err,
                    None,
                )?,
                cpal::SampleFormat::I32 => device.build_input_stream(
                    config,
                    move |d: &[i32], _: &_| {
                        receive(d.iter().map(|s| *s as f32 / 2147483648.0).collect())
                    },
                    err,
                    None,
                )?,
                format => bail!("unsupported capture sample format {format:?}"),
            };
            stream.play()?;
            Ok(stream)
        };
        match open() {
            Ok(stream) => {
                if tx.send(Ok(())).is_err() {
                    return;
                }
                while !end.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = tx.send(Err(format!("{e:#}")));
            }
        }
    });
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        other => {
            stop.store(true, Ordering::Release);
            // If device initialization is still blocked, its eventual ACK fails
            // and the worker drops the stream instead of retaining the device.
            match other {
                Ok(Err(e)) => {
                    let _ = thread.join();
                    return Err(anyhow::Error::msg(e));
                }
                Err(e) => return Err(e).context("audio device initialization timed out"),
                Ok(Ok(())) => unreachable!(),
            }
        }
    }
    Ok(Box::new(WindowsAudioSession {
        stop,
        thread: Some(thread),
        error,
    }))
}
