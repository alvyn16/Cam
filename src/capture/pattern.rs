//! Synthetic screen: a generated test pattern.

use std::sync::Arc;
use crate::capture::record::Shared;

pub struct Synth {}

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
    pub fn start(_w: usize, _h: usize, _fps: u32, _pix: u32, _noise: bool, _audio: bool, _shared: Arc<Shared>) -> Synth {
        Synth {}
    }
}
