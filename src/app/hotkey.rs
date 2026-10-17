//! Global hotkeys for Windows.

use std::cell::RefCell;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const KEY_R: u32 = 15;
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
        for (bit, glyph) in [(CONTROL, "Ctrl+"), (OPTION, "Alt+"), (SHIFT, "Shift+"), (CMD, "Win+")] {
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
        if m.mac_cmd || m.command {
            mods |= CMD;
        }
        if m.shift {
            mods |= SHIFT;
        }
        if m.alt {
            mods |= OPTION;
        }
        if m.ctrl && !m.command {
            mods |= CONTROL;
        }
        let is_fkey = K_F_KEYS.iter().any(|(k, _)| *k == code);
        (mods & (CMD | CONTROL | OPTION) != 0 || is_fkey).then_some(Combo { key: code, mods })
    }
}

thread_local! {
    static CALLBACKS: RefCell<HashMap<u32, Box<dyn Fn()>>> = RefCell::new(HashMap::new());
}

/// Bind `combo` to slot `id` (replacing any previous binding of that slot).
pub fn bind(id: u32, _combo: Combo, callback: impl Fn() + 'static) -> Result<(), i32> {
    unbind(id);
    CALLBACKS.with(|c| c.borrow_mut().insert(id, Box::new(callback)));
    Ok(())
}

/// Remove the binding of slot `id`, if any.
pub fn unbind(id: u32) {
    CALLBACKS.with(|c| c.borrow_mut().remove(&id));
}

/// Single-binding convenience.
pub fn register(key: u32, modifiers: u32, callback: impl Fn() + 'static) -> Result<(), i32> {
    bind(0, Combo::new(key, modifiers), callback)
}

const K_F_KEYS: [(u32, &str); 12] = [
    (122, "F1"),
    (120, "F2"),
    (99, "F3"),
    (118, "F4"),
    (96, "F5"),
    (97, "F6"),
    (98, "F7"),
    (100, "F8"),
    (101, "F9"),
    (109, "F10"),
    (103, "F11"),
    (111, "F12"),
];

/// Virtual key codes for letters and digits.
const LETTERS: [(u32, char); 36] = [
    (0, 'A'), (11, 'B'), (8, 'C'), (2, 'D'), (14, 'E'), (3, 'F'), (5, 'G'), (4, 'H'), (34, 'I'),
    (38, 'J'), (40, 'K'), (37, 'L'), (46, 'M'), (45, 'N'), (31, 'O'), (35, 'P'), (12, 'Q'), (15, 'R'),
    (1, 'S'), (17, 'T'), (32, 'U'), (9, 'V'), (13, 'W'), (7, 'X'), (16, 'Y'), (6, 'Z'), (29, '0'),
    (18, '1'), (19, '2'), (20, '3'), (21, '4'), (23, '5'), (22, '6'), (26, '7'), (28, '8'), (25, '9'),
];

fn key_name(code: u32) -> String {
    if let Some((_, c)) = LETTERS.iter().find(|(k, _)| *k == code) {
        return c.to_string();
    }
    if let Some((_, n)) = K_F_KEYS.iter().find(|(k, _)| *k == code) {
        return n.to_string();
    }
    match code {
        49 => "Space".into(),
        36 => "Enter".into(),
        53 => "Esc".into(),
        _ => format!("Key {code}"),
    }
}

fn egui_to_vk(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let c = match key {
        A => 'A', B => 'B', C => 'C', D => 'D', E => 'E', F => 'F', G => 'G', H => 'H',
        I => 'I', J => 'J', K => 'K', L => 'L', M => 'M', N => 'N', O => 'O', P => 'P',
        Q => 'Q', R => 'R', S => 'S', T => 'T', U => 'U', V => 'V', W => 'W', X => 'X',
        Y => 'Y', Z => 'Z',
        Num0 => '0', Num1 => '1', Num2 => '2', Num3 => '3', Num4 => '4',
        Num5 => '5', Num6 => '6', Num7 => '7', Num8 => '8', Num9 => '9',
        F1 => return Some(122), F2 => return Some(120), F3 => return Some(99),
        F4 => return Some(118), F5 => return Some(96), F6 => return Some(97),
        F7 => return Some(98), F8 => return Some(100), F9 => return Some(101),
        F10 => return Some(109), F11 => return Some(103), F12 => return Some(111),
        Space => return Some(49),
        Enter => return Some(36),
        Escape => return Some(53),
        _ => return None,
    };
    LETTERS.iter().find(|(_, ch)| *ch == c).map(|(k, _)| *k)
}
