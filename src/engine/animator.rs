//! Deterministic spring animation over a timeline.
//!
//! A scene node describes, for any instant, where it *wants* to be (its
//! target values) and which spring to use. The animator turns that into the
//! springy value at a given frame. To seek to frame `f` it restarts the
//! simulation a few seconds earlier, snapped to the target there, and steps
//! one frame at a time. Springs settle well within that window, so the frame
//! is effectively a pure function of time: scrubbing, playback and export
//! agree.

use std::collections::HashMap;

use super::spring::{NumberSpring, SpringConfig};

/// How far back a seek restarts the simulation.
pub const INFLUENCE_MS: f64 = 3500.0;
const MAX_CACHE: usize = 4096;

/// What a node wants at one instant.
#[derive(Clone, Debug)]
pub struct Target {
    pub values: Vec<f64>,
    pub spring: SpringConfig,
    /// Jump straight to the target (no animation) at this instant.
    pub snap: bool,
}

impl Target {
    pub fn new(values: Vec<f64>, spring: SpringConfig) -> Self {
        Self { values, spring, snap: false }
    }
}

pub struct Animator {
    fps: f64,
    springs: Vec<NumberSpring>,
    /// Frame the springs currently describe.
    at: Option<i64>,
    cache: HashMap<i64, Vec<f64>>,
}

impl Animator {
    pub fn new(fps: f64) -> Self {
        Self { fps, springs: vec![], at: None, cache: HashMap::new() }
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    pub fn frame_ms(&self, frame: i64) -> f64 {
        frame as f64 * 1000.0 / self.fps
    }

    /// Forget all state (call after the project changes).
    pub fn invalidate(&mut self) {
        self.at = None;
        self.cache.clear();
    }

    fn influence_frames(&self) -> i64 {
        (INFLUENCE_MS * self.fps / 1000.0).ceil() as i64
    }

    fn reset(&mut self, frame: i64, target: &mut impl FnMut(f64) -> Target) {
        let t = target(self.frame_ms(frame));
        self.springs = t.values.iter().map(|&v| NumberSpring::new(v, t.spring)).collect();
        self.at = Some(frame);
        self.cache.insert(frame, t.values);
    }

    /// Springy values at `frame`. `target` maps a timeline time in ms to the
    /// node's target.
    pub fn values_at(&mut self, frame: i64, mut target: impl FnMut(f64) -> Target) -> Vec<f64> {
        let frame = frame.max(0);
        if let Some(v) = self.cache.get(&frame) {
            return v.clone();
        }
        if self.cache.len() > MAX_CACHE {
            self.cache.clear();
        }
        let win = self.influence_frames();
        match self.at {
            Some(at) if frame >= at && frame <= at + win => {}
            _ => self.reset((frame - win).max(0), &mut target),
        }
        let mut f = self.at.unwrap();
        let dt = 1000.0 / self.fps;
        while f < frame {
            f += 1;
            let t = target(self.frame_ms(f));
            if t.values.len() != self.springs.len() {
                // The node's shape changed; restart from here.
                self.reset(f, &mut |_| t.clone());
                continue;
            }
            for (s, &v) in self.springs.iter_mut().zip(&t.values) {
                s.advance(dt);
                s.set_target(v);
                if t.snap {
                    s.snap_to_target();
                }
                s.set_config(t.spring);
            }
            self.cache.insert(f, self.springs.iter().map(|s| s.value).collect());
        }
        self.at = Some(frame);
        self.cache.get(&frame).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(t: f64) -> Target {
        Target::new(vec![if t >= 1000.0 { 100.0 } else { 0.0 }], SpringConfig::SCREEN_FOCUSED)
    }

    #[test]
    fn seeking_matches_sequential() {
        let mut seq = Animator::new(60.0);
        let mut last = vec![];
        for f in 0..=90 {
            last = seq.values_at(f, step);
        }
        let mut seek = Animator::new(60.0);
        let direct = seek.values_at(90, step);
        assert!((last[0] - direct[0]).abs() < 1e-9);
        // Mid-animation the value is between the endpoints.
        let mid = seek.values_at(66, step)[0];
        assert!(mid > 0.0 && mid < 100.0, "{mid}");
    }

    #[test]
    fn backwards_seek_recomputes() {
        let mut a = Animator::new(60.0);
        let late = a.values_at(300, step)[0];
        let early = a.values_at(30, step)[0];
        assert_eq!(early, 0.0);
        assert!((late - 100.0).abs() < 1e-6);
    }

    #[test]
    fn snap_jumps() {
        let mut a = Animator::new(60.0);
        let v = a.values_at(61, |t| Target { snap: t >= 1000.0, ..step(t) })[0];
        assert_eq!(v, 100.0);
    }
}
