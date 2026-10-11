//! Animated GIF export: frames are quantised to 256 colours in parallel
//! (NeuQuant) and written in order; identical frames merge into one.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use anyhow::{Context, Result, bail};

use super::{FramePump, Inputs, Job, ProgressFn};
use crate::engine::scene::Scene;
use crate::project::model::Quality;

pub(super) fn encode(
    job: &Job,
    scene: &mut Scene,
    pump: &mut FramePump,
    inputs: &mut Inputs,
    total: u64,
    progress: ProgressFn,
    cancel: &AtomicBool,
) -> Result<u64> {
    let s = job.settings;
    let (w, h) = (pump.w, pump.h);
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        bail!("GIF too large");
    }
    let file = File::create(&job.output).with_context(|| format!("creating {}", job.output.display()))?;
    let mut enc = gif::Encoder::new(BufWriter::new(file), w as u16, h as u16, &[])?;
    enc.set_repeat(if s.gif_loop { gif::Repeat::Infinite } else { gif::Repeat::Finite(0) })?;
    // Lower is slower and better; Studio spends more time per frame.
    let speed = if s.quality == Quality::Studio { 4 } else { 10 };
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(2);

    let (job_tx, job_rx) = mpsc::sync_channel::<(u64, Vec<u8>)>(workers * 2);
    let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
    let (out_tx, out_rx) = mpsc::channel::<(u64, gif::Frame<'static>)>();
    let mut threads = vec![];
    for _ in 0..workers {
        let (rx, tx) = (job_rx.clone(), out_tx.clone());
        threads.push(std::thread::spawn(move || {
            loop {
                let next = rx.lock().unwrap().recv();
                let Ok((i, mut rgba)) = next else { break };
                let frame = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut rgba, speed);
                if tx.send((i, frame)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(out_tx);

    // Frame timing in centiseconds, carrying rounding error forward.
    let frame_cs = 100.0 / s.fps as f64;
    let mut written = 0u64;
    let mut ready: BTreeMap<u64, gif::Frame<'static>> = BTreeMap::new();
    let mut held: Option<(gif::Frame<'static>, f64)> = None;
    let mut clock = 0.0f64;
    let mut emitted_cs = 0u64;
    let mut flush = |ready: &mut BTreeMap<u64, gif::Frame<'static>>,
                     held: &mut Option<(gif::Frame<'static>, f64)>,
                     enc: &mut gif::Encoder<BufWriter<File>>,
                     written: &mut u64|
     -> Result<()> {
        while let Some(f) = ready.remove(written) {
            *written += 1;
            match held {
                Some((prev, dur)) if prev.buffer == f.buffer && prev.palette == f.palette => *dur += frame_cs,
                _ => {
                    if let Some((mut prev, dur)) = held.take() {
                        clock += dur;
                        let end = clock.round() as u64;
                        prev.delay = (end - emitted_cs).max(2) as u16;
                        emitted_cs += prev.delay as u64;
                        enc.write_frame(&prev)?;
                    }
                    *held = Some((f, frame_cs));
                }
            }
        }
        Ok(())
    };

    let mut submitted = 0u64;
    let send_oldest = |pump: &mut FramePump, submitted: &mut u64| {
        if let Some(rgba) = pump.take(|rows, stride| {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h as usize {
                for px in rows[y * stride..y * stride + w as usize * 4].chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
            out
        }) {
            let _ = job_tx.send((*submitted, rgba));
            *submitted += 1;
        }
    };
    for i in 0..total {
        if cancel.load(Ordering::SeqCst) {
            drop(job_tx);
            let _ = std::fs::remove_file(&job.output);
            bail!("export cancelled");
        }
        let frame = scene.frame(i as f64 * 1000.0 / s.fps as f64, h as f64);
        inputs.feed(&mut pump.renderer, frame.source_ms, None);
        pump.submit(&frame, &job.project.style);
        if pump.full() {
            send_oldest(pump, &mut submitted);
        }
        while let Ok((idx, f)) = out_rx.try_recv() {
            ready.insert(idx, f);
        }
        flush(&mut ready, &mut held, &mut enc, &mut written)?;
        progress(i as f64 / total as f64 * 0.95);
    }
    while !pump.pending.is_empty() {
        send_oldest(pump, &mut submitted);
    }
    drop(job_tx);
    for (idx, f) in out_rx.iter() {
        ready.insert(idx, f);
        flush(&mut ready, &mut held, &mut enc, &mut written)?;
    }
    for t in threads {
        let _ = t.join();
    }
    if let Some((mut prev, dur)) = held.take() {
        clock += dur;
        prev.delay = ((clock.round() as u64).saturating_sub(emitted_cs)).max(2) as u16;
        enc.write_frame(&prev)?;
    }
    Ok(written)
}
