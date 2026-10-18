//! WASAPI process-excluding loopback (Windows build 20348 or later).
use anyhow::{Context, Result, ensure};
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};
use windows::{
    Win32::{Media::Audio::*, System::Com::*},
    core::{self as win_core, Interface, implement},
};

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Completion {
    sender: mpsc::SyncSender<std::result::Result<crate::util::SendBox<IAudioClient>, String>>,
    _params: Arc<crate::util::SendBox<win_core::PROPVARIANT>>,
}
impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
    fn ActivateCompleted(
        &self,
        operation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> win_core::Result<()> {
        let result = (|| -> win_core::Result<IAudioClient> {
            let operation = operation.ok_or_else(|| {
                win_core::Error::from_hresult(win_core::HRESULT(0x80004003u32 as i32))
            })?;
            let mut status = win_core::HRESULT(0);
            let mut object = None;
            unsafe {
                operation.GetActivateResult(&mut status, &mut object)?;
            }
            status.ok()?;
            object
                .ok_or_else(|| {
                    win_core::Error::from_hresult(win_core::HRESULT(0x80004003u32 as i32))
                })?
                .cast()
        })();
        // Callback and caller both run in the same COM MTA. IAudioClient is
        // transferred within that apartment, never into the GUI's STA.
        let _ = self
            .sender
            .try_send(result.map(crate::util::SendBox).map_err(|e| e.to_string()));
        Ok(())
    }
}

struct Apartment;
impl Apartment {
    fn initialize() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

fn activate(process: u32) -> Result<IAudioClient> {
    unsafe {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: process,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let size = std::mem::size_of_val(&params);
        let allocation = CoTaskMemAlloc(size) as *mut AUDIOCLIENT_ACTIVATION_PARAMS;
        ensure!(
            !allocation.is_null(),
            "allocating process loopback parameters"
        );
        allocation.write(params);
        let mut raw: win_core::imp::PROPVARIANT = std::mem::zeroed();
        raw.Anonymous.Anonymous.vt = 65; // VT_BLOB
        raw.Anonymous.Anonymous.Anonymous.blob = win_core::imp::BLOB {
            cbSize: size as u32,
            pBlobData: allocation.cast(),
        };
        // The completion handler retains the owned parameters even if our
        // wait times out before the asynchronous callback arrives.
        let params = Arc::new(crate::util::SendBox(win_core::PROPVARIANT::from_raw(raw)));
        let (tx, rx) = mpsc::sync_channel(1);
        let handler: IActivateAudioInterfaceCompletionHandler = Completion {
            sender: tx,
            _params: params.clone(),
        }
        .into();
        let _operation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&params.0),
            &handler,
        )
        .context(
            "activating process-excluding audio capture; Windows build 20348 or later is required",
        )?;
        let client = rx
            .recv_timeout(Duration::from_secs(4))
            .context("process loopback activation timed out")?
            .map_err(anyhow::Error::msg)?;
        Ok(client.0)
    }
}

struct Event(windows::Win32::Foundation::HANDLE);
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
struct RunningClient(IAudioClient);
impl Drop for RunningClient {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}

pub(super) fn start(
    mut callback: Box<dyn FnMut(&[f32], f64) + Send>,
) -> Result<Box<dyn crate::platform::audio::AudioStreamSession>> {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };
    let stop = Arc::new(AtomicBool::new(false));
    let end = stop.clone();
    let error = Arc::new(Mutex::new(None));
    let errors = error.clone();
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            let _apartment = Apartment::initialize()?;
            let client = activate(std::process::id())?;
            unsafe {
                use windows::Win32::{
                    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
                    System::{Performance::*, Threading::*},
                };
                let format = WAVEFORMATEX {
                    wFormatTag: 1,
                    nChannels: 2,
                    nSamplesPerSec: 48000,
                    nAvgBytesPerSec: 192000,
                    nBlockAlign: 4,
                    wBitsPerSample: 16,
                    cbSize: 0,
                };
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    0,
                    0,
                    &format,
                    None,
                )?;
                let event = Event(CreateEventW(None, false, false, None)?);
                client.SetEventHandle(event.0)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                let mut ticks = 0;
                let mut frequency = 0;
                QueryPerformanceCounter(&mut ticks)?;
                QueryPerformanceFrequency(&mut frequency)?;
                let host_offset = crate::util::host_now() - ticks as f64 / frequency as f64;
                client.Start()?;
                let _running = RunningClient(client);
                if tx.send(Ok(())).is_err() {
                    return Ok(());
                }
                while !end.load(Ordering::Acquire) {
                    match WaitForSingleObject(event.0, 50) {
                        WAIT_TIMEOUT => continue,
                        WAIT_OBJECT_0 => {}
                        _ => {
                            return Err(win_core::Error::from_win32())
                                .context("waiting for process-loopback audio");
                        }
                    }
                    while capture.GetNextPacketSize()? > 0 && !end.load(Ordering::Acquire) {
                        let mut data = std::ptr::null_mut();
                        let (mut frames, mut flags, mut qpc) = (0, 0, 0);
                        capture.GetBuffer(
                            &mut data,
                            &mut frames,
                            &mut flags,
                            None,
                            Some(&mut qpc),
                        )?;
                        let samples = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                            vec![0.0; frames as usize * 2]
                        } else {
                            ensure!(
                                !data.is_null(),
                                "process loopback returned an empty sample buffer"
                            );
                            std::slice::from_raw_parts(data as *const i16, frames as usize * 2)
                                .iter()
                                .map(|s| *s as f32 / 32768.0)
                                .collect()
                        };
                        capture.ReleaseBuffer(frames)?;
                        let pts = if flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0 {
                            qpc as f64 / 10_000_000.0 + host_offset
                        } else {
                            crate::util::host_now() - frames as f64 / 48000.0
                        };
                        callback(&samples, pts);
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            let message = format!("Process-excluding audio capture failed: {e:#}");
            *errors.lock().unwrap() = Some(message.clone());
            let _ = tx.send(Err(message));
        }
    });
    match rx.recv_timeout(Duration::from_secs(6)) {
        Ok(Ok(())) => Ok(Box::new(super::WindowsAudioSession {
            stop,
            thread: Some(thread),
            error,
        })),
        response => {
            stop.store(true, Ordering::Release);
            match response {
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(anyhow::Error::msg(e))
                }
                Err(e) => Err(e).context("starting process-excluding audio capture timed out"),
                Ok(Ok(())) => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "activates the Windows native process-loopback audio interface"]
    fn process_loopback_activation() {
        let _apartment = Apartment::initialize().unwrap();
        let _client = activate(std::process::id()).unwrap();
    }
    #[test]
    #[ignore = "captures native process-loopback audio on the connected PC"]
    fn process_loopback_delivers_stereo_frames_and_stops() {
        let samples = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = samples.clone();
        let mut stream = start(Box::new(move |data, pts| {
            assert_eq!(data.len() % 2, 0);
            assert!(pts.is_finite());
            count.fetch_add(data.len() / 2, std::sync::atomic::Ordering::Relaxed);
        }))
        .unwrap();
        std::thread::sleep(Duration::from_millis(400));
        stream.stop();
        assert!(stream.error().is_none(), "{:?}", stream.error());
        assert!(samples.load(std::sync::atomic::Ordering::Relaxed) >= 4800);
    }
}
