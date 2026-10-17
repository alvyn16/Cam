//! Windows input monitoring, cursor capture, and global hooks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::engine::geom::Rect;
use crate::platform::input::{InputBackend, InputLog, InputSession, RawKey, RawMouse};
use crate::project::recording::{CursorImage, MouseKind};

pub struct WindowsInputBackend;

impl WindowsInputBackend {
    pub fn new() -> Self {
        Self
    }
}

impl InputBackend for WindowsInputBackend {
    fn start_monitoring(&self, _track_window: Option<u32>, log_keys: bool) -> Result<Box<dyn InputSession>> {
        let stop = Arc::new(AtomicBool::new(false));
        let keys = Arc::new(Mutex::new(Vec::new()));

        let stop_clone = stop.clone();
        let poll_handle = std::thread::spawn(move || {
            let start = Instant::now();
            let mut mouse_log = Vec::new();
            let mut prev_left = false;
            let mut _prev_right = false;

            while !stop_clone.load(Ordering::Relaxed) {
                let mut pt = POINT::default();
                unsafe { let _ = GetCursorPos(&mut pt); };

                let left = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } < 0;
                let right = unsafe { GetAsyncKeyState(VK_RBUTTON.0 as i32) } < 0;

                let host = start.elapsed().as_secs_f64();

                let kind = if left && !prev_left {
                    MouseKind::Down
                } else if !left && prev_left {
                    MouseKind::Up
                } else {
                    MouseKind::Move
                };

                let button = if left { 0 } else if right { 1 } else { 0 };

                mouse_log.push(RawMouse {
                    host,
                    x: pt.x as f64,
                    y: pt.y as f64,
                    kind,
                    button,
                    cursor: 0,
                });

                prev_left = left;
                _prev_right = right;

                std::thread::sleep(Duration::from_millis(8)); // ~120 Hz
            }

            mouse_log
        });

        Ok(Box::new(WindowsInputSession {
            stop,
            poll_handle: Some(poll_handle),
            keys,
            log_keys,
        }))
    }

    fn current_cursor_image(&self) -> Option<CursorImage> {
        let mut ci = CURSORINFO::default();
        ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;

        unsafe {
            if GetCursorInfo(&mut ci).is_ok() && ci.flags == CURSOR_SHOWING {
                let mut ii = ICONINFOEXW::default();
                ii.cbSize = std::mem::size_of::<ICONINFOEXW>() as u32;
                if GetIconInfoExW(HICON(ci.hCursor.0), &mut ii).as_bool() {
                    let hot_x = ii.xHotspot as f64;
                    let hot_y = ii.yHotspot as f64;

                    if !ii.hbmColor.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
                    }
                    if !ii.hbmMask.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
                    }

                    return Some(CursorImage {
                        id: 0,
                        file: None,
                        hotspot: (hot_x / 32.0, hot_y / 32.0),
                        size: (32.0, 32.0),
                        system: Some("arrow".into()),
                    });
                }
            }
        }
        None
    }

    fn display_bounds(&self, _display_id: u32) -> Rect {
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        Rect::new(0.0, 0.0, w as f64, h as f64)
    }
}

pub struct WindowsInputSession {
    stop: Arc<AtomicBool>,
    poll_handle: Option<JoinHandle<Vec<RawMouse>>>,
    keys: Arc<Mutex<Vec<RawKey>>>,
    log_keys: bool,
}

impl InputSession for WindowsInputSession {
    fn finish(mut self: Box<Self>) -> InputLog {
        self.stop.store(true, Ordering::Relaxed);
        let mouse = self.poll_handle.take().and_then(|h| h.join().ok()).unwrap_or_default();
        let keys = std::mem::take(&mut *self.keys.lock().unwrap());

        InputLog {
            mouse,
            keys,
            cursors: vec![],
            bounds: vec![],
            keys_enabled: self.log_keys,
        }
    }
}
