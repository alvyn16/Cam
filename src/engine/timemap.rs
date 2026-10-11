//! Mapping between playback time (the edited video) and source time (the
//! recording). The edit is an ordered list of kept slices; gaps are cuts and
//! each slice may play faster or slower.

use crate::project::model::Slice;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub pb_start: f64,
    pub pb_end: f64,
    pub src_start: f64,
    pub src_end: f64,
    pub speed: f64,
    /// Index into the project's slice list.
    pub slice: usize,
}

#[derive(Clone, Debug, Default)]
pub struct TimeMap {
    pub links: Vec<Link>,
}

impl TimeMap {
    /// Build from slices; with no slices the whole recording plays once.
    pub fn new(slices: &[Slice], duration_ms: f64) -> Self {
        let mut order: Vec<usize> = (0..slices.len()).collect();
        order.sort_by(|&a, &b| slices[a].start.total_cmp(&slices[b].start));
        let mut links = vec![];
        let mut pb = 0.0;
        for i in order {
            let s = &slices[i];
            let (start, end) = (s.start.max(0.0), s.end.min(duration_ms));
            if end <= start {
                continue;
            }
            let speed = if s.speed > 0.0 { s.speed } else { 1.0 };
            let len = (end - start) / speed;
            links.push(Link { pb_start: pb, pb_end: pb + len, src_start: start, src_end: end, speed, slice: i });
            pb += len;
        }
        if links.is_empty() && duration_ms > 0.0 {
            links.push(Link {
                pb_start: 0.0,
                pb_end: duration_ms,
                src_start: 0.0,
                src_end: duration_ms,
                speed: 1.0,
                slice: usize::MAX,
            });
        }
        TimeMap { links }
    }

    pub fn duration(&self) -> f64 {
        self.links.last().map_or(0.0, |l| l.pb_end)
    }

    pub fn link_at_playback(&self, pb: f64) -> Option<&Link> {
        if self.links.is_empty() {
            return None;
        }
        let i = self.links.partition_point(|l| l.pb_end <= pb);
        Some(&self.links[i.min(self.links.len() - 1)])
    }

    /// Source time shown at a playback time.
    pub fn to_source(&self, pb: f64) -> f64 {
        match self.link_at_playback(pb) {
            Some(l) => (l.src_start + (pb - l.pb_start).max(0.0) * l.speed).min(l.src_end),
            None => pb,
        }
    }

    /// Playback time of a source time, or None if that moment was cut.
    pub fn to_playback(&self, src: f64) -> Option<f64> {
        self.links
            .iter()
            .find(|l| src >= l.src_start && src <= l.src_end)
            .map(|l| l.pb_start + (src - l.src_start) / l.speed)
    }

    /// Nearest playback time for a source time (cut moments snap to the next kept frame).
    pub fn to_playback_nearest(&self, src: f64) -> f64 {
        if let Some(t) = self.to_playback(src) {
            return t;
        }
        for l in &self.links {
            if src < l.src_start {
                return l.pb_start;
            }
        }
        self.duration()
    }

    pub fn speed_at_playback(&self, pb: f64) -> f64 {
        self.link_at_playback(pb).map_or(1.0, |l| l.speed)
    }

    pub fn source_start(&self) -> f64 {
        self.links.first().map_or(0.0, |l| l.src_start)
    }

    pub fn source_end(&self) -> f64 {
        self.links.iter().map(|l| l.src_end).fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(start: f64, end: f64, speed: f64) -> Slice {
        Slice { start, end, speed, ..Default::default() }
    }

    #[test]
    fn identity_without_slices() {
        let m = TimeMap::new(&[], 5000.0);
        assert_eq!(m.duration(), 5000.0);
        assert_eq!(m.to_source(1234.0), 1234.0);
    }

    #[test]
    fn cuts_and_speed() {
        // Keep 0..1000 at 1x, cut 1000..3000, keep 3000..5000 at 2x.
        let m = TimeMap::new(&[slice(3000.0, 5000.0, 2.0), slice(0.0, 1000.0, 1.0)], 6000.0);
        assert_eq!(m.duration(), 2000.0);
        assert_eq!(m.to_source(500.0), 500.0);
        assert_eq!(m.to_source(1000.0), 3000.0);
        assert_eq!(m.to_source(1500.0), 4000.0);
        assert_eq!(m.to_playback(2000.0), None);
        assert_eq!(m.to_playback(4000.0), Some(1500.0));
        assert_eq!(m.to_playback_nearest(2000.0), 1000.0);
        assert_eq!(m.speed_at_playback(1999.0), 2.0);
    }
}
