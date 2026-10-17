//! Real-time audio mixer: system audio + microphone -> one stereo track.
//!
//! Each source's buffers are placed on a common 48 kHz timeline.

use std::collections::VecDeque;
use crate::capture::writer::AudioKind;

pub const RATE: f64 = 48000.0;
const MAX_WAIT: i64 = 24000;
const SNAP: i64 = 96;

pub struct Mixer {
    t0: i64,
    base: i64,
    buf: VecDeque<[f32; 2]>,
    horizon: [Option<i64>; 2],
    active: [bool; 2],
    gain: [f32; 2],
    pub clipped: u64,
}

unsafe impl Send for Mixer {}

fn idx(kind: AudioKind) -> usize {
    match kind {
        AudioKind::System => 0,
        AudioKind::Mic => 1,
    }
}

impl Mixer {
    pub fn new(t0_secs: f64, system: bool, mic: bool, gain_db: [f32; 2]) -> Self {
        Mixer {
            t0: (t0_secs * RATE).round() as i64,
            base: 0,
            buf: VecDeque::new(),
            horizon: [None, None],
            active: [system, mic],
            gain: gain_db.map(|db| 10f32.powf(db / 20.0)),
            clipped: 0,
        }
    }

    pub fn push(&mut self, kind: AudioKind, pts: f64, rate: f64, frames: Vec<[f32; 2]>) {
        let frames = resample(frames, rate);
        let num_frames = frames.len();
        let i = idx(kind);
        let mut start = (pts * RATE).round() as i64 - self.t0;
        if let Some(h) = self.horizon[i]
            && (start - h).abs() <= SNAP {
            start = h;
        }
        let g = self.gain[i];
        for (k, fr) in frames.into_iter().enumerate() {
            let pos = start + k as i64;
            if pos < self.base {
                continue;
            }
            let off = (pos - self.base) as usize;
            if off >= self.buf.len() {
                self.buf.resize(off + 1, [0.0, 0.0]);
            }
            self.buf[off][0] += fr[0] * g;
            self.buf[off][1] += fr[1] * g;
        }
        self.horizon[i] = Some(start + num_frames as i64);
    }
}

pub fn resample(frames: Vec<[f32; 2]>, rate: f64) -> Vec<[f32; 2]> {
    if (rate - RATE).abs() < 1.0 || frames.is_empty() {
        return frames;
    }
    let ratio = RATE / rate;
    let out_len = ((frames.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx0 = src_pos.floor() as usize;
        let frac = (src_pos - idx0 as f64) as f32;
        let s0 = frames[idx0.min(frames.len() - 1)];
        let s1 = frames[(idx0 + 1).min(frames.len() - 1)];
        out.push([
            s0[0] + (s1[0] - s0[0]) * frac,
            s0[1] + (s1[1] - s0[1]) * frac,
        ]);
    }
    out
}
