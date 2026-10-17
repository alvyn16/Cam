//! Windows recording session orchestration.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;

use crate::capture::compose::PipSpec;
use crate::capture::screen;
use crate::capture::writer::{Codec, Stats};

#[derive(Clone, Debug)]
pub struct Options {
    pub output: PathBuf,
    pub size: (usize, usize),
    pub fps: u32,
    pub codec: Codec,
    pub bitrate: u64,
    pub quality: Option<f64>,
    pub system_audio: bool,
    pub mic: Option<Option<String>>,
    pub webcam: Option<Option<String>>,
    pub pip: PipSpec,
    pub hide_desktop_icons: bool,
    pub hide_dock: bool,
    pub exclude_own_audio: bool,
    pub mic_backend: MicBackend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Default)]
pub enum MicBackend {
    #[default]
    Auto,
    Sck,
    Avcapture,
}

pub struct Shared {
    pub sink: Mutex<Option<crate::capture::writer::Sink>>,
    pub error: Mutex<Option<String>>,
    pub stop: Arc<AtomicBool>,
}

impl Shared {
    pub fn with<R>(&self, f: impl FnOnce(&mut crate::capture::writer::Sink) -> R) -> Option<R> {
        let mut g = self.sink.lock().ok()?;
        g.as_mut().map(f)
    }
}

pub struct Session {
    pub output: Option<PathBuf>,
    pub started: Instant,
    pub description: String,
}

impl Session {
    pub fn start(
        _target: screen::Target,
        o: Options,
    ) -> Result<Session> {
        Ok(Session {
            output: Some(o.output),
            started: Instant::now(),
            description: "Windows Screen".into(),
        })
    }

    pub fn pause(&self, _now: f64) -> bool {
        true
    }

    pub fn resume(&self, _now: f64) -> bool {
        true
    }

    pub fn is_paused(&self) -> bool {
        false
    }

    pub fn failed(&self) -> bool {
        false
    }

    pub fn error(&self) -> Option<String> {
        None
    }

    pub fn t0_secs(&self) -> Option<f64> {
        Some(0.0)
    }

    pub fn recorded_secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn has_frames(&self) -> bool {
        true
    }

    pub fn poll(&self) -> (Stats, [f32; 2]) {
        (Stats::default(), [0.0, 0.0])
    }

    pub fn finish(self) -> Result<Stats> {
        Ok(Stats::default())
    }
}

pub fn check_permissions(_o: &Options) -> Result<()> {
    Ok(())
}

pub fn clock(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}
