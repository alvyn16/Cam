//! The floating recording control bar on Windows.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

/// What the bar shows.
#[derive(Clone, PartialEq)]
pub enum State {
    /// Seconds left before capture starts.
    Countdown(u64),
    Recording { clock: String, paused: bool },
}

pub struct RecordBar {
    origin: Point,
}

impl RecordBar {
    pub fn new(_display: u32, origin: Option<Point>, _stop_hint: &str, _pause_hint: &str) -> RecordBar {
        RecordBar { origin: origin.unwrap_or_default() }
    }

    pub fn update(&mut self, _state: State) {}

    pub fn origin(&self) -> Point {
        self.origin
    }

    pub fn window_number(&self) -> isize {
        0
    }

    pub fn draggable(&self) -> bool {
        true
    }
}
