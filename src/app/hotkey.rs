//! Global hotkeys via Carbon `RegisterEventHotKey`. Unlike event taps this
//! needs no Accessibility permission; callbacks run on the main thread.
//! Several shortcuts can be bound, each re-bindable at runtime.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use serde::{Deserialize, Serialize};

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: Handler,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: *mut c_void) -> i32;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> i32;
}

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const SIGNATURE: u32 = u32::from_be_bytes(*b"GLD1");

pub const KEY_R: u32 = 15; // kVK_ANSI_R
pub const CMD: u32 = 1 << 8; // cmdKey
pub const SHIFT: u32 = 1 << 9; // shiftKey
pub const OPTION: u32 = 1 << 11; // optionKey
pub const CONTROL: u32 = 1 << 12; // controlKey

/// A key combination: macOS virtual key code + Carbon modifier flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Combo {
    pub key: u32,
    pub mods: u32,
}

impl Combo {
    pub const fn new(key: u32, mods: u32) -> Self {
        Combo { key, mods }
    }

    /// "⌥⇧⌘R" style label.
    pub fn label(&self) -> String {
        let mut s = String::new();
        for (bit, glyph) in [(CONTROL, "⌃"), (OPTION, "⌥"), (SHIFT, "⇧"), (CMD, "⌘")] {
            if self.mods & bit != 0 {
                s.push_str(glyph);
            }
        }
        s.push_str(&key_name(self.key));
        s
    }

    /// From an egui key press; None for keys we can't map or combos
    /// without ⌘, ⌃ or ⌥ (plain keys would hijack typing).
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
        let is_fkey = (K_F_KEYS).iter().any(|(k, _)| *k == code);
        (mods & (CMD | CONTROL | OPTION) != 0 || is_fkey).then_some(Combo { key: code, mods })
    }
}

thread_local! {
    static CALLBACKS: RefCell<HashMap<u32, Box<dyn Fn()>>> = RefCell::new(HashMap::new());
    static REFS: RefCell<HashMap<u32, usize>> = RefCell::new(HashMap::new());
    static INSTALLED: RefCell<bool> = const { RefCell::new(false) };
}

extern "C" fn on_hotkey(_next: *mut c_void, event: *mut c_void, _user: *mut c_void) -> i32 {
    let mut id = EventHotKeyID::default();
    // SAFETY: reading the hot key id parameter Carbon delivers with the event.
    let st = unsafe {
        GetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            std::ptr::null_mut(),
            std::mem::size_of::<EventHotKeyID>(),
            std::ptr::null_mut(),
            &mut id as *mut _ as *mut c_void,
        )
    };
    if st == 0 && id.signature == SIGNATURE {
        CALLBACKS.with(|c| {
            if let Some(f) = c.borrow().get(&id.id) {
                f();
            }
        });
    }
    0
}

/// Bind `combo` to slot `id` (replacing any previous binding of that slot).
/// Must be called on the main thread. Fails if another app owns the combo.
pub fn bind(id: u32, combo: Combo, callback: impl Fn() + 'static) -> Result<(), i32> {
    unbind(id);
    unsafe {
        let target = GetApplicationEventTarget();
        if !INSTALLED.with(|i| *i.borrow()) {
            let spec = EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind: K_EVENT_HOT_KEY_PRESSED };
            let mut handler_ref = std::ptr::null_mut();
            let st = InstallEventHandler(target, on_hotkey, 1, &spec, std::ptr::null_mut(), &mut handler_ref);
            if st != 0 {
                return Err(st);
            }
            INSTALLED.with(|i| *i.borrow_mut() = true);
        }
        let mut hk = std::ptr::null_mut();
        let st = RegisterEventHotKey(combo.key, combo.mods, EventHotKeyID { signature: SIGNATURE, id }, target, 0, &mut hk);
        if st != 0 {
            return Err(st);
        }
        REFS.with(|r| r.borrow_mut().insert(id, hk as usize));
    }
    CALLBACKS.with(|c| c.borrow_mut().insert(id, Box::new(callback)));
    Ok(())
}

/// Remove the binding of slot `id`, if any.
pub fn unbind(id: u32) {
    if let Some(hk) = REFS.with(|r| r.borrow_mut().remove(&id)) {
        // SAFETY: `hk` came from RegisterEventHotKey and is unregistered once.
        unsafe { UnregisterEventHotKey(hk as *mut c_void) };
    }
    CALLBACKS.with(|c| c.borrow_mut().remove(&id));
}

/// Single-binding convenience used before settings existed.
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

/// macOS ANSI virtual key codes for letters and digits.
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
        36 => "↩".into(),
        53 => "⎋".into(),
        _ => format!("key {code}"),
    }
}

fn egui_to_vk(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let name = key.name();
    if name.len() == 1 {
        let c = name.chars().next()?.to_ascii_uppercase();
        return LETTERS.iter().find(|(_, l)| *l == c).map(|(k, _)| *k);
    }
    let f = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    if let Some(i) = f.iter().position(|k| *k == key) {
        return Some(K_F_KEYS[i].0);
    }
    match key {
        Space => Some(49),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_egui_mapping() {
        assert_eq!(Combo::new(KEY_R, CMD | SHIFT | OPTION).label(), "⌥⇧⌘R");
        let m = egui::Modifiers { alt: true, shift: true, command: true, mac_cmd: true, ..Default::default() };
        assert_eq!(Combo::from_egui(egui::Key::R, m), Some(Combo::new(KEY_R, CMD | SHIFT | OPTION)));
        // Plain letters are rejected; F-keys alone are fine.
        assert_eq!(Combo::from_egui(egui::Key::R, egui::Modifiers::default()), None);
        assert_eq!(Combo::from_egui(egui::Key::F5, egui::Modifiers::default()), Some(Combo::new(96, 0)));
        assert_eq!(Combo::new(23, CONTROL).label(), "⌃5");
    }
}
