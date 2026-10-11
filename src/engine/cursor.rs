//! The re-rendered cursor: where it should be, how it tilts, presses, hides
//! and which spring moves it. Pure functions of source time.

use super::events::Events;
use super::geom::{Vec2, keyframes, remap};
use super::spring::{NumberSpring, SpringConfig};
use crate::project::model::{CursorStyle, Slice};
use crate::project::recording::MouseKind;

/// Base cursor height in design pixels at size 1.
pub const BASE_HEIGHT: f64 = 40.0;
/// Horizontal travel window used for tilt.
const TILT_WINDOW_MS: f64 = 400.0;
const TILT_GAIN: f64 = 0.03;
const TILT_MAX_DEG: f64 = 20.0;
/// The cursor starts shrinking this long before a press.
const PRESS_LEAD_MS: f64 = 130.0;
pub const PRESS_SCALE: f64 = 0.8;
/// A hidden idle cursor reappears this long before it moves.
const IDLE_REVEAL_MS: f64 = 250.0;

/// Fade/press state: (alpha, scale, blur).
pub const VISIBLE: [f64; 3] = [1.0, 1.0, 0.0];
pub const HIDDEN: [f64; 3] = [0.0, 0.8, 5.0];

pub struct CursorModel<'a> {
    pub events: &'a Events,
    pub style: &'a CursorStyle,
    pub slices: &'a [Slice],
    /// Source range that is played (for loop/freeze-at-end).
    pub source_start: f64,
    pub source_end: f64,
}

impl CursorModel<'_> {
    fn slice_at(&self, t: f64) -> Option<&Slice> {
        self.slices.iter().find(|s| t >= s.start && t <= s.end)
    }

    fn smoothing_on(&self, t: f64) -> bool {
        self.style.smoothing.is_some() && !self.slice_at(t).is_some_and(|s| s.no_smoothing)
    }

    /// Time after which the cursor no longer follows the recording
    /// (it glides home or freezes), with the position it holds.
    fn held(&self) -> Option<(f64, Vec2)> {
        if let Some(ms) = self.style.loop_ms.filter(|m| *m > 0.0) {
            let at = (self.source_end - ms).max(self.source_start);
            return Some((at, self.raw_target(self.source_start)?));
        }
        if self.style.freeze_ms > 0.0 {
            let at = (self.source_end - self.style.freeze_ms).max(self.source_start);
            return Some((at, self.raw_target(at)?));
        }
        None
    }

    fn is_held(&self, t: f64) -> bool {
        self.held().is_some_and(|(at, _)| t > at)
    }

    /// Where the cursor heads at `t` before springs: onto an imminent click
    /// point (removes shake), otherwise the last recorded position.
    fn raw_target(&self, t: f64) -> Option<Vec2> {
        if self.smoothing_on(t)
            && let Some(c) = Events::first_from(&self.events.clicks, t).filter(|c| c.t - t <= self.style.click_snap_ms)
        {
            return Some(Vec2 { x: c.x, y: c.y });
        }
        self.events.position_at(t)
    }

    pub fn target(&self, t: f64) -> Option<Vec2> {
        if let Some((at, p)) = self.held()
            && t > at
        {
            return Some(p);
        }
        self.raw_target(t)
    }

    /// Tilt in degrees from recent horizontal travel.
    pub fn tilt(&self, t: f64) -> f64 {
        let ratio = if self.style.smoothing.is_some() { self.style.tilt } else { 0.0 };
        let (Some(now), Some(before)) = (self.target(t), self.target(t - TILT_WINDOW_MS)) else {
            return self.style.base_rotation;
        };
        ((now.x - before.x) * TILT_GAIN * ratio).clamp(-TILT_MAX_DEG, TILT_MAX_DEG) + self.style.base_rotation
    }

    /// Spring that moves the cursor toward its target at `t`.
    pub fn spring(&self, t: f64) -> SpringConfig {
        let Some(base) = self.style.smoothing else { return SpringConfig::INSTANT };
        if !self.smoothing_on(t) {
            return SpringConfig::INSTANT;
        }
        if Events::first_from(&self.events.clicks, t).is_some_and(|c| c.t - t <= self.style.click_snap_ms) {
            return SpringConfig::NEAR_CLICK;
        }
        if Events::last_at(&self.events.all, t).is_some_and(|e| e.kind == MouseKind::Drag) {
            return SpringConfig::DRAG;
        }
        base
    }

    /// (alpha, scale, blur) target for presses and idle hiding.
    pub fn fade(&self, t: f64) -> [f64; 3] {
        if self.slice_at(t).is_some_and(|s| s.hide_cursor) {
            return HIDDEN;
        }
        if self.is_held(t) {
            return VISIBLE;
        }
        let mut out = VISIBLE;
        if let Some(c) = Events::first_from(&self.events.clicks, t) {
            let pressing = (c.kind == MouseKind::Down && c.t - t <= PRESS_LEAD_MS) || c.kind == MouseKind::Up;
            if pressing {
                out[1] = PRESS_SCALE;
            }
        }
        if let Some(idle) = self.style.hide_idle_ms.filter(|m| *m > 0.0) {
            let moves = &self.events.all;
            if let Some(first) = moves.first()
                && first.t > idle
                && first.t > t
            {
                return HIDDEN;
            }
            let next_soon = Events::first_from(moves, t).is_some_and(|e| e.t - t < IDLE_REVEAL_MS);
            if !next_soon && Events::last_at(moves, t).is_some_and(|e| t - e.t > idle) {
                return HIDDEN;
            }
        }
        out
    }
}

/// One visible click effect at time `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClickFx {
    pub pos: Vec2,
    /// Seconds-like progress 0..1 over the effect's life.
    pub progress: f64,
    pub alpha: f64,
    pub scale: f64,
    pub blur: f64,
}

pub const CIRCLE_MS: f64 = 150.0;
const CIRCLE_TAIL_MS: f64 = 450.0;
pub const RIPPLE_MS: f64 = 250.0;

/// Expanding-circle effects alive at `t`. Keyframes are smoothed with a
/// small spring simulated from the click, so the result stays a pure
/// function of time.
pub fn circle_effects(events: &Events, t: f64) -> Vec<ClickFx> {
    let downs = Events::range(&events.downs, t - CIRCLE_TAIL_MS, t);
    downs
        .iter()
        .map(|d| {
            let curve = |p: f64| {
                [
                    keyframes(p, &[0.0, 0.05, 0.8, 1.0], &[0.0, 1.0, 0.0, 0.0]),
                    keyframes(p, &[0.0, 1.0], &[0.2, 3.5]),
                    keyframes(p, &[0.0, 0.05, 0.8, 1.0], &[0.0, 0.0, 0.0, 20.0]),
                ]
            };
            let start = curve(0.0);
            let mut springs = start.map(|v| NumberSpring::new(v, SpringConfig::CLICK));
            let mut now = d.t;
            let step: f64 = 1000.0 / 120.0;
            while now < t {
                let dt = step.min(t - now);
                now += dt;
                let target = curve(remap(now - d.t, 0.0, CIRCLE_MS, 0.0, 1.0));
                for (s, v) in springs.iter_mut().zip(target) {
                    s.advance(dt);
                    s.set_target(v);
                }
            }
            ClickFx {
                pos: Vec2 { x: d.x, y: d.y },
                progress: ((t - d.t) / CIRCLE_TAIL_MS).clamp(0.0, 1.0),
                alpha: springs[0].value.clamp(0.0, 1.0),
                scale: springs[1].value.max(0.0),
                blur: springs[2].value.max(0.0),
            }
        })
        .filter(|fx| fx.alpha > 0.002)
        .collect()
}

/// Ripple (shockwave) effects alive at `t`.
pub fn ripple_effects(events: &Events, t: f64) -> Vec<ClickFx> {
    Events::range(&events.downs, t - RIPPLE_MS, t)
        .iter()
        .map(|d| {
            let p = ((t - d.t) / RIPPLE_MS).clamp(0.0, 1.0);
            ClickFx { pos: Vec2 { x: d.x, y: d.y }, progress: p, alpha: 1.0 - p, scale: p, blur: 0.0 }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::recording::MouseEvent;

    fn ev(t: f64, x: f64, kind: MouseKind) -> MouseEvent {
        MouseEvent { t, x, y: 100.0, kind, button: 0, cursor: 0 }
    }

    fn model<'a>(events: &'a Events, style: &'a CursorStyle) -> CursorModel<'a> {
        CursorModel { events, style, slices: &[], source_start: 0.0, source_end: 10_000.0 }
    }

    #[test]
    fn glides_onto_upcoming_click() {
        let events = Events::new(vec![
            ev(0.0, 0.0, MouseKind::Move),
            ev(100.0, 50.0, MouseKind::Move),
            ev(400.0, 300.0, MouseKind::Down),
            ev(450.0, 300.0, MouseKind::Up),
        ]);
        let style = CursorStyle::default();
        let m = model(&events, &style);
        // 0 ms: the click at 400 ms is within 500 ms, so head to it.
        assert_eq!(m.target(0.0).unwrap().x, 300.0);
        // Without smoothing, follow the raw position.
        let raw = CursorStyle { smoothing: None, ..style };
        assert_eq!(model(&events, &raw).target(150.0).unwrap().x, 50.0);
    }

    #[test]
    fn tilt_follows_horizontal_travel() {
        let events = Events::new(vec![ev(0.0, 0.0, MouseKind::Move), ev(500.0, 400.0, MouseKind::Move)]);
        let style = CursorStyle { click_snap_ms: 0.0, ..Default::default() };
        let m = model(&events, &style);
        let a = m.tilt(600.0);
        assert!(a > 0.0 && a <= 20.0, "{a}");
        assert_eq!(m.tilt(2000.0), 0.0);
    }

    #[test]
    fn press_and_idle() {
        let events = Events::new(vec![
            ev(0.0, 0.0, MouseKind::Move),
            ev(1000.0, 0.0, MouseKind::Down),
            ev(1100.0, 0.0, MouseKind::Up),
        ]);
        let style = CursorStyle { hide_idle_ms: Some(2000.0), ..Default::default() };
        let m = model(&events, &style);
        assert_eq!(m.fade(900.0)[1], PRESS_SCALE);
        assert_eq!(m.fade(1050.0)[1], PRESS_SCALE);
        assert_eq!(m.fade(500.0), VISIBLE);
        assert_eq!(m.fade(4000.0), HIDDEN);
    }

    #[test]
    fn circle_effect_lifecycle() {
        let events = Events::new(vec![ev(1000.0, 10.0, MouseKind::Down)]);
        assert!(circle_effects(&events, 990.0).is_empty());
        let mid = circle_effects(&events, 1060.0);
        assert_eq!(mid.len(), 1);
        assert!(mid[0].alpha > 0.1 && mid[0].scale > 0.5, "{:?}", mid[0]);
        assert!(circle_effects(&events, 1500.0).is_empty());
    }
}
