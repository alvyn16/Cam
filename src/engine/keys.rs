//! Keyboard shortcut badges: which key presses are worth showing and how
//! repeated presses group together.

use crate::project::recording::{KeyEvent, mods};

/// Identical presses closer than this extend one badge.
const GROUP_MS: f64 = 2000.0;
/// A badge stays this long after its last press.
pub const HOLD_MS: f64 = 1500.0;
/// Shift+key followed this quickly by plain typing is just typing.
const TYPING_MS: f64 = 1000.0;

/// Keys worth showing even without modifiers.
const SPECIAL: [&str; 13] = [
    "⏎", "⎋", "⇥", "⌫", "⌦", "←", "→", "↑", "↓", "Space", "Home", "End", "⇞",
];

fn special(key: &str) -> bool {
    SPECIAL.contains(&key)
        || cfg!(target_os = "windows")
            && matches!(
                key,
                "Enter"
                    | "Esc"
                    | "Escape"
                    | "Tab"
                    | "Backspace"
                    | "Delete"
                    | "Left"
                    | "Right"
                    | "Up"
                    | "Down"
                    | "Page Up"
                    | "Page Down"
                    | "Insert"
            )
}
/// Ignore modifier-only events from older Windows recordings as well as new
/// captures. Text names avoid confusing legacy Mac scan codes with Windows VKs.
pub(crate) fn modifier_only(e: &KeyEvent) -> bool {
    #[cfg(target_os = "windows")]
    if matches!(e.code, 0x10..=0x12 | 0x5b | 0x5c | 0xa0..=0xa5) {
        return [
            "Ctrl",
            "Control",
            "Shift",
            "Alt",
            "Left Ctrl",
            "Right Ctrl",
            "Left Shift",
            "Right Shift",
            "Left Alt",
            "Right Alt",
            "Left Windows",
            "Right Windows",
        ]
        .iter()
        .any(|name| e.key.eq_ignore_ascii_case(name));
    }
    false
}

#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub label: String,
    pub start: f64,
    pub end: f64,
    pub count: usize,
}

pub fn label(e: &KeyEvent) -> String {
    let mut s = String::new();
    #[cfg(target_os = "windows")]
    let modifiers = [
        (mods::CTRL, "Ctrl+"),
        (mods::OPT, "Alt+"),
        (mods::SHIFT, "Shift+"),
        (mods::CMD, "Win+"),
        (mods::FN, "Fn+"),
    ];
    #[cfg(not(target_os = "windows"))]
    let modifiers = [
        (mods::CTRL, "⌃"),
        (mods::OPT, "⌥"),
        (mods::SHIFT, "⇧"),
        (mods::CMD, "⌘"),
        (mods::FN, "fn "),
    ];
    for (bit, glyph) in modifiers {
        if e.mods & bit != 0 {
            s.push_str(glyph);
        }
    }
    s.push_str(&e.key);
    s
}

fn modifier_count(e: &KeyEvent) -> u32 {
    (e.mods & !mods::FN).count_ones()
}

/// Turn raw key events into badges.
pub fn badges(events: &[KeyEvent], single_keys: bool) -> Vec<Badge> {
    let downs: Vec<&KeyEvent> = events
        .iter()
        .filter(|e| e.down && !e.key.is_empty() && !modifier_only(e))
        .collect();
    let mut shown: Vec<&KeyEvent> = vec![];
    for (i, e) in downs.iter().enumerate() {
        let n = modifier_count(e);
        let plain = n == 0 || (e.mods & !(mods::SHIFT | mods::FN)) == 0;
        let keep = if single_keys {
            true
        } else if n == 0 {
            special(&e.key)
        } else if plain {
            // Shift+letter: show only when not part of typing.
            let typing = downs
                .get(i + 1)
                .is_some_and(|nx| modifier_count(nx) == 0 && nx.t - e.t < TYPING_MS);
            !typing || special(&e.key)
        } else {
            true
        };
        if keep {
            shown.push(e);
        }
    }
    let mut out: Vec<Badge> = vec![];
    for e in shown {
        let l = label(e);
        match out.last_mut() {
            Some(b) if b.label == l && e.t - (b.end - HOLD_MS) <= GROUP_MS => {
                b.end = e.t + HOLD_MS;
                b.count += 1;
            }
            _ => out.push(Badge {
                label: l,
                start: e.t,
                end: e.t + HOLD_MS,
                count: 1,
            }),
        }
    }
    // A new badge replaces the previous one.
    for i in 1..out.len() {
        if out[i - 1].end > out[i].start {
            out[i - 1].end = out[i].start;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_badges_use_native_labels_and_skip_legacy_modifier_rows() {
        let mut ctrl = key(0.0, "Ctrl", mods::CTRL);
        ctrl.code = 0x11;
        let mut left_ctrl = ctrl.clone();
        left_ctrl.code = 0xa2;
        let events = [
            ctrl,
            left_ctrl,
            key(10.0, "C", mods::CTRL),
            key(2000.0, "Enter", 0),
            key(4000.0, "Page Down", 0),
        ];
        let badges = badges(&events, false);
        assert_eq!(
            badges.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(),
            ["Ctrl+C", "Enter", "Page Down"]
        );
        let mut legacy_letter = key(0.0, "Y", 0);
        legacy_letter.code = 0x10;
        assert!(!modifier_only(&legacy_letter));
    }

    fn key(t: f64, k: &str, m: u8) -> KeyEvent {
        KeyEvent {
            t,
            code: 0,
            key: k.into(),
            mods: m,
            down: true,
        }
    }

    #[test]
    fn shows_shortcuts_not_typing() {
        let ev = [
            key(0.0, "H", 0),
            key(100.0, "I", 0),
            key(1000.0, "S", mods::CMD),
            key(1500.0, "S", mods::CMD),
            key(5000.0, "⏎", 0),
            key(8000.0, "C", mods::CMD | mods::SHIFT),
        ];
        let b = badges(&ev, false);
        let labels: Vec<_> = b.iter().map(|b| b.label.as_str()).collect();
        #[cfg(target_os = "windows")]
        assert_eq!(labels, ["Win+S", "⏎", "Shift+Win+C"]);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(labels, ["⌘S", "⏎", "⇧⌘C"]);
        assert_eq!(b[0].count, 2);
        assert_eq!(b[0].end, 1500.0 + HOLD_MS);
    }

    #[test]
    fn single_keys_option() {
        let b = badges(&[key(0.0, "A", 0)], true);
        assert_eq!(b.len(), 1);
    }
}
