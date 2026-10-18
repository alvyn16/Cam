//! Windows input logging: mouse coordinates, click events, cursor bitmaps, and keys.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    keys_enabled: Arc<AtomicBool>,
}

fn modifier(code: u16) -> bool {
    matches!(code, 0x10..=0x12 | 0x5b | 0x5c | 0xa0..=0xa5)
}
fn key_name(code: u16) -> String {
    let special = match code {
        0x08 => Some("Backspace"),
        0x09 => Some("Tab"),
        0x0d => Some("Enter"),
        0x1b => Some("Esc"),
        0x20 => Some("Space"),
        0x21 => Some("Page Up"),
        0x22 => Some("Page Down"),
        0x23 => Some("End"),
        0x24 => Some("Home"),
        0x25 => Some("Left"),
        0x26 => Some("Up"),
        0x27 => Some("Right"),
        0x28 => Some("Down"),
        0x2d => Some("Insert"),
        0x2e => Some("Delete"),
        _ => None,
    };
    if let Some(name) = special {
        return name.into();
    }
    if (0x70..=0x87).contains(&code) {
        return format!("F{}", code - 0x6f);
    }
    unsafe {
        let scan = MapVirtualKeyW(code as u32, MAPVK_VK_TO_VSC);
        let mut buf = [0u16; 128];
        let n = GetKeyNameTextW((scan << 16) as i32, &mut buf);
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            format!("Key {code}")
        }
    }
}

pub fn display_bounds(id: u32) -> Rect {
    let (_, x, y, w, h) = crate::app::os::display_frame(id);
    Rect::new(x, y, w, h)
}

pub fn window_bounds(id: u32) -> Option<Rect> {
    crate::platform::windows::ensure_dpi_awareness();
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
    pub fn start(cursor_dir: PathBuf, window: Option<u32>, log_keys: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let keys_enabled = Arc::new(AtomicBool::new(log_keys));

        let stop_clone = stop.clone();
        let poll = std::thread::spawn(move || {
            crate::platform::windows::ensure_dpi_awareness();
            let mut log = InputLog::default();
            log.keys_enabled = log_keys;
            let mut buttons = [false; 2];
            let mut key_states = [false; 256];
            let mut cursor_ids = std::collections::HashMap::new();
            let mut cursor_id = 0;
            let mut last_cursor = 0usize;
            let mut last_pos: Option<(f64, f64)> = None;

            while !stop_clone.load(Ordering::SeqCst) {
                let now = secs(host_now());
                unsafe {
                    let mut ci = CURSORINFO::default();
                    ci.cbSize = std::mem::size_of_val(&ci) as u32;
                    if GetCursorInfo(&mut ci).is_ok() && ci.hCursor.0 as usize != last_cursor {
                        last_cursor = ci.hCursor.0 as usize;
                        cursor_id = if let Some(id) = cursor_ids.get(&last_cursor) {
                            *id
                        } else {
                            let id = log.cursors.len() as u32;
                            if let Some((handle, image)) = super::cursor::current(&cursor_dir, id) {
                                cursor_ids.insert(handle, id);
                                log.cursors.push(image);
                            }
                            id
                        };
                    }
                    if log_keys {
                        use crate::project::recording::mods;
                        let mut modifiers = 0;
                        for (vk, bit) in [
                            (VK_CONTROL, mods::CTRL),
                            (VK_SHIFT, mods::SHIFT),
                            (VK_MENU, mods::OPT),
                            (VK_LWIN, mods::CMD),
                            (VK_RWIN, mods::CMD),
                        ] {
                            if GetAsyncKeyState(vk.0 as i32) < 0 {
                                modifiers |= bit;
                            }
                        }
                        for code in 8..256u16 {
                            let down = GetAsyncKeyState(code as i32) < 0;
                            if down != key_states[code as usize] {
                                key_states[code as usize] = down;
                                if modifier(code) {
                                    continue;
                                }
                                let key = key_name(code);
                                log.keys.push(RawKey {
                                    host: now,
                                    code,
                                    key,
                                    mods: modifiers,
                                    down,
                                });
                            }
                        }
                    }
                }
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                };
                let (px, py) = (pt.x as f64, pt.y as f64);

                let left = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } < 0;
                let right = unsafe { GetAsyncKeyState(VK_RBUTTON.0 as i32) } < 0;
                let pressed = [left, right];

                for b in 0..2 {
                    if pressed[b] != buttons[b] {
                        let kind = if pressed[b] {
                            MouseKind::Down
                        } else {
                            MouseKind::Up
                        };
                        log.mouse.push(RawMouse {
                            host: now,
                            x: px,
                            y: py,
                            kind,
                            button: b as u8,
                            cursor: cursor_id,
                        });
                        buttons[b] = pressed[b];
                    }
                }

                if last_pos != Some((px, py)) {
                    let kind = if buttons[0] {
                        MouseKind::Drag
                    } else {
                        MouseKind::Move
                    };
                    log.mouse.push(RawMouse {
                        host: now,
                        x: px,
                        y: py,
                        kind,
                        button: 0,
                        cursor: cursor_id,
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
            keys_enabled,
        }
    }

    pub fn stop(mut self) -> InputLog {
        self.stop.store(true, Ordering::SeqCst);
        let log = self
            .poll
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();

        log
    }

    pub fn keys_enabled(&self) -> bool {
        self.keys_enabled.load(Ordering::Relaxed)
    }
}

impl Drop for InputLogger {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.poll.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_modifiers_and_extended_keys_have_unambiguous_names() {
        for code in [
            0x10, 0x11, 0x12, 0x5b, 0x5c, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5,
        ] {
            assert!(modifier(code));
        }
        for code in [0x08, 0x0d, 0x20, 0x24, 0x41, 0x70, 0x87] {
            assert!(!modifier(code));
        }
        assert_eq!(key_name(0x25), "Left");
        assert_eq!(key_name(0x2e), "Delete");
        assert_eq!(key_name(0x0d), "Enter");
        assert_eq!(key_name(0x87), "F24");
    }
}
