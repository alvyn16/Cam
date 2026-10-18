//! Windows input logging: mouse coordinates, click events, cursor bitmaps, and keys.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::engine::geom::Rect;
use crate::project::recording::{CursorImage, MouseKind};
use crate::util::*;

#[derive(Clone, Copy, Debug)]
pub struct RawMouse {
    pub host: f64,
    pub x: f64,
    pub y: f64,
    pub kind: MouseKind,
    pub button: u8,
    pub cursor: u32,
}

#[derive(Clone, Debug)]
pub struct RawKey {
    pub host: f64,
    pub code: u16,
    pub key: String,
    pub mods: u8,
    pub down: bool,
}

#[derive(Default)]
pub struct InputLog {
    pub mouse: Vec<RawMouse>,
    pub keys: Vec<RawKey>,
    pub cursors: Vec<CursorImage>,
    pub bounds: Vec<(f64, Rect)>,
    pub keys_enabled: bool,
}

pub struct InputLogger {
    stop: Arc<AtomicBool>,
    poll: Option<JoinHandle<InputLog>>,
    keys: Arc<Mutex<Vec<RawKey>>>,
    keys_enabled: Arc<AtomicBool>,
}

pub fn display_bounds(_id: u32) -> Rect {
    let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    Rect::new(0.0, 0.0, w as f64, h as f64)
}

pub fn window_bounds(id: u32) -> Option<Rect> {
    let hwnd = HWND(id as _);
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
        Some(Rect::new(
            rect.left as f64,
            rect.top as f64,
            (rect.right - rect.left).max(0) as f64,
            (rect.bottom - rect.top).max(0) as f64,
        ))
    } else {
        None
    }
}

impl InputLogger {
    pub fn start(_cursor_dir: PathBuf, window: Option<u32>, log_keys: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let keys = Arc::new(Mutex::new(vec![]));
        let keys_enabled = Arc::new(AtomicBool::new(log_keys));

        let stop_clone = stop.clone();
        let poll = std::thread::spawn(move || {
            let mut log = InputLog::default();
            log.keys_enabled = log_keys;
            let mut buttons = [false; 2];
            let mut last_pos: Option<(f64, f64)> = None;

            while !stop_clone.load(Ordering::SeqCst) {
                let now = secs(host_now());
                let mut pt = POINT::default();
                unsafe { let _ = GetCursorPos(&mut pt); };
                let (px, py) = (pt.x as f64, pt.y as f64);

                let left = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } < 0;
                let right = unsafe { GetAsyncKeyState(VK_RBUTTON.0 as i32) } < 0;
                let pressed = [left, right];

                for b in 0..2 {
                    if pressed[b] != buttons[b] {
                        let kind = if pressed[b] { MouseKind::Down } else { MouseKind::Up };
                        log.mouse.push(RawMouse {
                            host: now,
                            x: px,
                            y: py,
                            kind,
                            button: b as u8,
                            cursor: 0,
                        });
                        buttons[b] = pressed[b];
                    }
                }

                if last_pos != Some((px, py)) {
                    let kind = if buttons[0] { MouseKind::Drag } else { MouseKind::Move };
                    log.mouse.push(RawMouse {
                        host: now,
                        x: px,
                        y: py,
                        kind,
                        button: 0,
                        cursor: 0,
                    });
                    last_pos = Some((px, py));
                }

                if let Some(w) = window {
                    if let Some(b) = window_bounds(w) {
                        if log.bounds.last().is_none_or(|(_, last)| *last != b) {
                            log.bounds.push((now, b));
                        }
                    }
                }

                std::thread::sleep(Duration::from_micros(8_333)); // ~120 Hz
            }

            log
        });

        InputLogger {
            stop,
            poll: Some(poll),
            keys,
            keys_enabled,
        }
    }

    pub fn stop(mut self) -> InputLog {
        self.stop.store(true, Ordering::SeqCst);
        let mut log = self.poll.take().and_then(|h| h.join().ok()).unwrap_or_default();
        log.keys = std::mem::take(&mut *self.keys.lock().unwrap());
        log
    }

    pub fn keys_enabled(&self) -> bool {
        self.keys_enabled.load(Ordering::Relaxed)
    }
}
