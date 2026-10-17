//! Menu bar status item / system tray.

/// What the menu bar shows.
pub struct View<'a> {
    pub recording: bool,
    pub paused: bool,
    pub finishing: bool,
    pub clock: &'a str,
}

pub struct MenuBar;

impl MenuBar {
    pub fn new() -> MenuBar {
        MenuBar
    }

    pub fn set_hotkey(&self, _label: &str, _ok: bool) {}

    pub fn title(&self) -> Option<String> {
        None
    }

    pub fn update(&mut self, _v: &View) {}
}
