//! Windows WASAPI capture. cpal enables loopback on an output device.
mod convert;
mod dispatch;
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
    callback: Box<dyn FnMut(&[f32], f64) + Send>,
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
        let open = || -> Result<(cpal::Stream, dispatch::Processor)> {
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
            let mut config = supported.config();
            // WASAPI's default ring on this Realtek input is only 10 ms.
            // Shared-mode buffer capacity does not change its 10-ms callback
            // period; retain 100 ms so brief game/driver scheduling stalls do
            // not overwrite audio before we can drain the packets.
            config.buffer_size = cpal::BufferSize::Fixed((config.sample_rate / 10).max(1));
            let n = config.channels as usize;
            anyhow::ensure!(n > 0 && config.sample_rate > 0, "invalid capture format");
            // CPAL's WASAPI capture timestamp is the QPC time of the first
            // sample. Map that clock once, rather than using callback arrival
            // time (callbacks often drain several queued packets under load).
            let host_offset = qpc_host_offset()?;
            let processor =
                dispatch::Processor::new(config.sample_rate, n, callback, errors.clone());
            let input = processor.input();
            let receive = move |data: Vec<f32>, info: &cpal::InputCallbackInfo| {
                let capture = info.timestamp().capture.as_nanos() as f64 / 1e9 + host_offset;
                input.push(data, capture);
            };
            let err = move |e: cpal::Error| {
                // Never write to a potentially blocked stderr pipe from the
                // device thread, including during recoverable startup xruns.
                if !matches!(
                    e.kind(),
                    cpal::ErrorKind::Xrun
                        | cpal::ErrorKind::DeviceChanged
                        | cpal::ErrorKind::RealtimeDenied
                ) {
                    *errors.lock().unwrap() = Some(format!("Audio capture device failed: {e}"));
                }
            };
            use cpal::SampleFormat as F;
            let stream = match supported.sample_format() {
                F::F32 => capture_stream::<f32>(&device, config, receive, err)?,
                F::F64 => capture_stream::<f64>(&device, config, receive, err)?,
                F::I8 => capture_stream::<i8>(&device, config, receive, err)?,
                F::I16 => capture_stream::<i16>(&device, config, receive, err)?,
                F::I24 => capture_stream::<cpal::I24>(&device, config, receive, err)?,
                F::I32 => capture_stream::<i32>(&device, config, receive, err)?,
                F::I64 => capture_stream::<i64>(&device, config, receive, err)?,
                F::U8 => capture_stream::<u8>(&device, config, receive, err)?,
                F::U16 => capture_stream::<u16>(&device, config, receive, err)?,
                F::U32 => capture_stream::<u32>(&device, config, receive, err)?,
                F::U64 => capture_stream::<u64>(&device, config, receive, err)?,
                format => bail!("unsupported capture sample format {format:?}"),
            };
            stream.play()?;
            // Starting a CPAL stream only queues its start command. Wait for
            // real, steady microphone packets before starting the video clock.
            // Loopback may legitimately deliver nothing while speakers are idle.
            if !loopback {
                processor.wait_ready()?;
            }
            // Tuple fields drop in order: stop the native stream and release
            // its queue sender before joining the processor's drain.
            Ok((stream, processor))
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

fn capture_stream<T: cpal::SizedSample>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut receive: impl FnMut(Vec<f32>, &cpal::InputCallbackInfo) + Send + 'static,
    error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream>
where
    f32: cpal::FromSample<T>,
{
    Ok(device.build_input_stream(
        config,
        move |samples: &[T], info| {
            receive(
                samples
                    .iter()
                    .map(|sample| sample.to_sample::<f32>())
                    .collect(),
                info,
            );
        },
        error,
        None,
    )?)
}

fn qpc_host_offset() -> Result<f64> {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut counter, mut frequency) = (0, 0);
    unsafe {
        QueryPerformanceFrequency(&mut frequency)?;
        QueryPerformanceCounter(&mut counter)?;
    }
    Ok(crate::util::host_now() - counter as f64 / frequency as f64)
}
