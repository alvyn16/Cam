//! Synthetic screen: a generated test pattern.

use crate::capture::record::Shared;
use std::sync::Arc;

pub struct Synth {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub const BEEP_SECS: f64 = 0.04;

pub fn beep_samples(start: i64, n: usize) -> Vec<f32> {
    let rate = crate::capture::mixer::RATE as i64;
    let len = (BEEP_SECS * rate as f64) as i64;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n as i64 {
        let s = start + i;
        let in_sec = s.rem_euclid(rate);
        let sample = if in_sec < len {
            let t = in_sec as f32 / rate as f32;
            (2.0 * std::f32::consts::PI * 1000.0 * t).sin() * 0.2
        } else {
            0.0
        };
        out.push(sample);
        out.push(sample);
    }
    out
}

impl Synth {
    pub fn start(
        w: usize,
        h: usize,
        fps: u32,
        _pix: u32,
        _noise: bool,
        _audio: bool,
        shared: Arc<Shared>,
    ) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let end = stop.clone();
        let thread = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            while !end.load(std::sync::atomic::Ordering::Acquire)
                && !shared.stop.load(std::sync::atomic::Ordering::Acquire)
            {
                let t = std::time::Instant::now();
                let bgra = crate::synth::screen_bgra(
                    w as u32,
                    h as u32,
                    start.elapsed().as_secs_f64() * 1000.0,
                );
                let image = super::grab::Image {
                    width: w as u32,
                    height: h as u32,
                    bgra,
                };
                shared.with(|s| s.push_video(&image));
                std::thread::sleep(
                    std::time::Duration::from_secs_f64(1.0 / fps.max(1) as f64)
                        .saturating_sub(t.elapsed()),
                );
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Synth {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
