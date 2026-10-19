//! Constant-rate recording from WGC textures into the hardware encoder.
use super::{audio::Clock, screen::Target, writer::Stats};
use crate::media::{mf_encoder::NativeWriter, writer::WriterSpec};
use anyhow::{Context, Result, ensure};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

pub(super) struct State {
    pub clock: Arc<Mutex<Clock>>,
    pub stop: Arc<AtomicBool>,
    pub cancelled: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    pub frames: Arc<AtomicU64>,
}
pub(super) fn start(
    target: &Target,
    path: &Path,
    spec: WriterSpec,
    state: State,
) -> Result<JoinHandle<Result<Stats>>> {
    let capture = super::wgc::Stream::start_gpu(target, spec.fps, false)?;
    let first = capture.snapshot_gpu()?;
    let device = unsafe { first.texture.GetDevice()? };
    let path = path.to_owned();
    let (ready, started) = mpsc::sync_channel(1);
    let (accept, accepted) = mpsc::sync_channel(0);
    let worker = std::thread::spawn(move || {
        let result = (|| -> Result<Stats> {
            let mut writer = match NativeWriter::create_for_capture(&path, &spec, device) {
                Ok(writer) => writer,
                Err(error) => {
                    let _ = ready.send(Err(format!("{error:#}")));
                    return Err(error);
                }
            };
            ready
                .send(Ok(()))
                .map_err(|_| anyhow::anyhow!("GPU capture startup abandoned"))?;
            accepted.recv().context("GPU capture startup abandoned")?;
            eprintln!(
                "Cam GPU capture: {} (no full-frame CPU readback)",
                writer.encoder_name()
            );
            drop(first);
            while !state.stop.load(Ordering::Acquire) {
                let now = crate::util::host_now();
                let elapsed = {
                    let mut clock = state.clock.lock().unwrap();
                    if clock.pause.is_some() {
                        drop(clock);
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    clock.start.get_or_insert(now);
                    clock.elapsed(now)
                };
                let wanted = (elapsed * spec.fps as f64).floor() as u64 + 1;
                if state.frames.load(Ordering::Relaxed) >= wanted {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                }
                let frame = capture.snapshot_gpu()?;
                while state.frames.load(Ordering::Relaxed) < wanted {
                    ensure!(
                        !state.cancelled.load(Ordering::Acquire),
                        "recording cancelled"
                    );
                    writer.push_texture(&frame.texture, &|| {
                        state.cancelled.load(Ordering::Acquire)
                    })?;
                    state.frames.fetch_add(1, Ordering::Release);
                }
            }
            ensure!(
                !state.cancelled.load(Ordering::Acquire),
                "recording cancelled"
            );
            let n = state.frames.load(Ordering::Acquire);
            ensure!(n > 0, "no GPU screen frames captured");
            // Release capture before closing the encoder; no callbacks retain
            // textures/device contexts while the native writer drains.
            drop(capture);
            writer.finish_cancellable(&|| state.cancelled.load(Ordering::Acquire))?;
            Ok(Stats {
                screen_frames_in: n,
                video_frames_out: n,
                ..Default::default()
            })
        })();
        if let Err(error) = &result {
            *state.error.lock().unwrap() = Some(format!("{error:#}"));
        }
        result
    });
    match started.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(Ok(())) => {
            accept.send(()).context("accepting GPU capture")?;
            Ok(worker)
        }
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(anyhow::anyhow!(error))
        }
        Err(error) => Err(error).context("GPU capture initialization timed out"),
    }
}
