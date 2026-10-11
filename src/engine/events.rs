//! Time-indexed access to recorded mouse events.

use super::geom::{Vec2, v2};
use crate::project::recording::{MouseEvent, MouseKind};

#[derive(Clone, Debug, Default)]
pub struct Events {
    /// Every mouse event (all kinds carry a position), sorted by time.
    pub all: Vec<MouseEvent>,
    /// Button presses and releases.
    pub clicks: Vec<MouseEvent>,
    /// Button presses only.
    pub downs: Vec<MouseEvent>,
}

impl Events {
    pub fn new(mut events: Vec<MouseEvent>) -> Self {
        events.sort_by(|a, b| a.t.total_cmp(&b.t));
        let clicks: Vec<_> =
            events.iter().copied().filter(|e| matches!(e.kind, MouseKind::Down | MouseKind::Up)).collect();
        let downs = clicks.iter().copied().filter(|e| e.kind == MouseKind::Down).collect();
        Events { all: events, clicks, downs }
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// Last event at or before `t`.
    pub fn last_at(list: &[MouseEvent], t: f64) -> Option<&MouseEvent> {
        let i = list.partition_point(|e| e.t <= t);
        if i == 0 { None } else { Some(&list[i - 1]) }
    }

    /// First event at or after `t`.
    pub fn first_from(list: &[MouseEvent], t: f64) -> Option<&MouseEvent> {
        let i = list.partition_point(|e| e.t < t);
        list.get(i)
    }

    /// Events with `t0 <= t <= t1`.
    pub fn range(list: &[MouseEvent], t0: f64, t1: f64) -> &[MouseEvent] {
        let a = list.partition_point(|e| e.t < t0);
        let b = list.partition_point(|e| e.t <= t1);
        &list[a..b.max(a)]
    }

    /// Raw mouse position at `t` (last known, or the first ever).
    pub fn position_at(&self, t: f64) -> Option<Vec2> {
        Self::last_at(&self.all, t).or(self.all.first()).map(|e| v2(e.x, e.y))
    }

    /// Next press within `window` ms after `t`.
    pub fn upcoming_click(&self, t: f64, window: f64) -> Option<&MouseEvent> {
        Self::first_from(&self.downs, t).filter(|e| e.t - t <= window)
    }
}
