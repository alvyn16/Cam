//! Background "data may be visible" checks for the editor: re-scan the
//! recording under blur/pixelate masks after they change and keep the
//! flagged ranges for the timeline.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::exposure::{self, Exposure};
use crate::project::model::Mask;

pub struct MaskWatch {
    video: PathBuf,
    cache: Arc<Mutex<exposure::Cache>>,
    /// Hiding masks as of the last request.
    masks: Vec<Mask>,
    /// When the next scan should start (edits settle first).
    due: Option<Instant>,
    latest: Arc<AtomicU64>,
    running: Option<(u64, JoinHandle<Vec<Exposure>>)>,
    /// Flagged ranges for the current masks (source ms).
    pub found: Vec<Exposure>,
}

impl MaskWatch {
    pub fn new(video: PathBuf) -> Self {
        MaskWatch {
            video,
            cache: Arc::default(),
            masks: vec![],
            due: None,
            latest: Arc::new(AtomicU64::new(0)),
            running: None,
            found: vec![],
        }
    }

    /// Call every tick with the project's masks. Returns true when `found`
    /// changed (redraw).
    pub fn update(&mut self, masks: &[Mask]) -> bool {
        let hiding: Vec<Mask> = masks.iter().filter(|m| exposure::hides(m.kind)).copied().collect();
        let mut changed = false;
        if hiding != self.masks {
            // Keep results for masks whose area and range are unchanged.
            let same = |id: u64| {
                let (a, b) = (self.masks.iter().find(|m| m.id == id), hiding.iter().find(|m| m.id == id));
                matches!((a, b), (Some(a), Some(b)) if a.rect == b.rect && a.start == b.start && a.end == b.end)
            };
            let before = self.found.len();
            self.found.retain(|e| same(e.mask));
            changed = self.found.len() != before;
            self.masks = hiding;
            self.latest.fetch_add(1, Ordering::SeqCst);
            self.due = Some(Instant::now() + Duration::from_millis(400));
        }
        if let Some((generation, h)) = self.running.take_if(|(_, h)| h.is_finished()) {
            let found = h.join().unwrap_or_default();
            if generation == self.latest.load(Ordering::SeqCst) && found != self.found {
                self.found = found;
                changed = true;
            }
        }
        if self.running.is_none() && self.due.is_some_and(|t| Instant::now() >= t) {
            self.due = None;
            let generation = self.latest.load(Ordering::SeqCst);
            let (video, masks, cache, latest) = (self.video.clone(), self.masks.clone(), self.cache.clone(), self.latest.clone());
            let h = std::thread::spawn(move || {
                let cancelled = || latest.load(Ordering::SeqCst) != generation;
                let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
                exposure::scan(&video, &masks, &mut cache, &cancelled).unwrap_or_default()
            });
            self.running = Some((generation, h));
        }
        changed
    }

    /// A scan is waiting or running.
    pub fn busy(&self) -> bool {
        self.due.is_some() || self.running.is_some()
    }

    /// Stop any scan in progress (leaving the editor).
    pub fn cancel(&mut self) {
        self.latest.fetch_add(1, Ordering::SeqCst);
        self.due = None;
    }
}
