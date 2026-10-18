//! Global hotkeys for Windows.

use std::cell::RefCell;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const KEY_R: u32 = 0x52;
pub const CMD: u32 = 1 << 8;
pub const SHIFT: u32 = 1 << 9;
pub const OPTION: u32 = 1 << 11;
pub const CONTROL: u32 = 1 << 12;

/// A key combination: virtual key code + modifier flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Combo {
    pub key: u32,
    pub mods: u32,
}

impl Combo {
    pub const fn new(key: u32, mods: u32) -> Self {
        Combo { key, mods }
    }

    /// "Ctrl+Shift+R" style label.
    pub fn label(&self) -> String {
        let mut s = String::new();
        for (bit, glyph) in [
            (CONTROL, "Ctrl+"),
            (OPTION, "Alt+"),
            (SHIFT, "Shift+"),
            (CMD, "Win+"),
        ] {
            if self.mods & bit != 0 {
                s.push_str(glyph);
            }
        }
        s.push_str(&key_name(self.key));
        s
    }

    /// From an egui key press.
    pub fn from_egui(key: egui::Key, m: egui::Modifiers) -> Option<Combo> {
        let code = egui_to_vk(key)?;
        let mut mods = 0;
        if m.mac_cmd {
            mods |= CMD;
        }
        if m.shift {
            mods |= SHIFT;
        }
        if m.alt {
            mods |= OPTION;
        }
        if m.ctrl {
            mods |= CONTROL;
        }
        let is_fkey = K_F_KEYS.iter().any(|(k, _)| *k == code);
        (mods & (CMD | CONTROL | OPTION) != 0 || is_fkey).then_some(Combo { key: code, mods })
    }
}

struct Binding {
    callback: Box<dyn Fn()>,
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread_id: u32,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Binding {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id,
                windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
thread_local! {static CALLBACKS:RefCell<HashMap<u32,Binding>>=RefCell::new(HashMap::new());}
pub fn bind(id: u32, combo: Combo, callback: impl Fn() + 'static) -> Result<(), i32> {
    unbind(id);
    let pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = pending.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        use windows::Win32::{
            System::Threading::GetCurrentThreadId,
            UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
        };
        let mut mods = MOD_NOREPEAT;
        for (bit, win) in [
            (CONTROL, MOD_CONTROL),
            (SHIFT, MOD_SHIFT),
            (OPTION, MOD_ALT),
            (CMD, MOD_WIN),
        ] {
            if combo.mods & bit != 0 {
                mods |= win;
            }
        }
        unsafe {
            let mut message = MSG::default();
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
            match RegisterHotKey(None, id as i32, mods, combo.key) {
                Ok(()) => {
                    let _ = tx.send(Ok(GetCurrentThreadId()));
                }
                Err(e) => {
                    let _ = tx.send(Err(e.code().0));
                    return;
                }
            }
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                if message.message == WM_HOTKEY {
                    flag.store(true, std::sync::atomic::Ordering::Release);
                    super::wake();
                }
            }
            let _ = UnregisterHotKey(None, id as i32);
        }
    });
    let thread_id = rx.recv().map_err(|_| -1)??;
    CALLBACKS.with(|c| {
        c.borrow_mut().insert(
            id,
            Binding {
                callback: Box::new(callback),
                pending,
                thread_id,
                thread: Some(thread),
            },
        )
    });
    Ok(())
}
pub fn poll() {
    CALLBACKS.with(|c| {
        for b in c.borrow().values() {
            if b.pending.swap(false, std::sync::atomic::Ordering::Acquire) {
                (b.callback)();
            }
        }
    });
}
pub fn unbind(id: u32) {
    CALLBACKS.with(|c| c.borrow_mut().remove(&id));
}

/// Single-binding convenience.
pub fn register(key: u32, modifiers: u32, callback: impl Fn() + 'static) -> Result<(), i32> {
    bind(0, Combo::new(key, modifiers), callback)
}

const K_F_KEYS: [(u32, &str); 12] = [
    (112, "F1"),
    (113, "F2"),
    (114, "F3"),
    (115, "F4"),
    (116, "F5"),
    (117, "F6"),
    (118, "F7"),
    (119, "F8"),
    (120, "F9"),
    (121, "F10"),
    (122, "F11"),
    (123, "F12"),
];
fn key_name(code: u32) -> String {
    if (0x30..=0x5a).contains(&code) {
        return char::from_u32(code).unwrap().to_string();
    }
    if let Some((_, n)) = K_F_KEYS.iter().find(|(k, _)| *k == code) {
        return n.to_string();
    }
    match code {
        32 => "Space".into(),
        13 => "Enter".into(),
        27 => "Esc".into(),
        _ => format!("Key {code}"),
    }
}
fn egui_to_vk(key: egui::Key) -> Option<u32> {
    let name = format!("{key:?}");
    if name.len() == 1 {
        return Some(name.as_bytes()[0] as u32);
    }
    if let Some(d) = name.strip_prefix("Num") {
        return d.parse::<u32>().ok().map(|n| 0x30 + n);
    }
    if let Some(f) = name.strip_prefix('F').and_then(|s| s.parse::<u32>().ok()) {
        if (1..=12).contains(&f) {
            return Some(111 + f);
        }
    }
    match key {
        egui::Key::Space => Some(32),
        egui::Key::Enter => Some(13),
        egui::Key::Escape => Some(27),
        _ => None,
    }
}

/// Release registrations while Windows worker threads are still running.
pub fn shutdown() {
    CALLBACKS.with(|callbacks| callbacks.borrow_mut().clear());
}

/// Migrate pre-port settings, which stored Apple key codes even on Windows.
pub fn migrate_legacy(mut combo: Combo) -> Combo {
    let original = combo;
    let codes = [
        0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16,
        6, 29, 18, 19, 20, 21, 23, 22, 26, 28, 25,
    ];
    if let Some(i) = codes.iter().position(|k| *k == combo.key) {
        combo.key = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"[i] as u32;
    } else {
        combo.key = match combo.key {
            49 => 32,
            36 => 13,
            53 => 27,
            122 => 112,
            120 => 113,
            99 => 114,
            118 => 115,
            96 => 116,
            97 => 117,
            98 => 118,
            100 => 119,
            101 => 120,
            109 => 121,
            103 => 122,
            111 => 123,
            key => key,
        };
    }
    if combo.mods & CMD != 0 {
        combo.mods = (combo.mods & !CMD) | CONTROL;
    }
    if [15, 35].contains(&original.key) && original.mods == CMD | SHIFT | OPTION {
        combo.mods = CONTROL | SHIFT;
    }
    combo
}
#[cfg(test)]
mod migration_tests {
    use super::*;
    #[test]
    fn old_defaults_become_windows_defaults() {
        assert_eq!(
            migrate_legacy(Combo::new(15, CMD | SHIFT | OPTION)),
            Combo::new(0x52, CONTROL | SHIFT)
        );
        assert_eq!(
            migrate_legacy(Combo::new(35, CMD | SHIFT | OPTION)),
            Combo::new(0x50, CONTROL | SHIFT)
        );
        assert_eq!(
            migrate_legacy(Combo::new(8, CMD)),
            Combo::new(0x43, CONTROL)
        );
        assert_eq!(migrate_legacy(Combo::new(122, 0)), Combo::new(112, 0));
    }
}
