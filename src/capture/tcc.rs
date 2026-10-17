//! Windows permission stub (Windows has no macOS-style TCC restrictions for screen capture).

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "granted")
    }
}

pub fn disclaim_and_reexec() {}

pub fn screen_status() -> Status {
    Status::Granted
}

pub fn mic_status() -> Status {
    Status::Granted
}

pub fn camera_status() -> Status {
    Status::Granted
}

pub fn accessibility_status() -> Status {
    Status::Granted
}

pub fn request_all(_screen: bool, _mic: bool, _camera: bool, _timeout: Duration) -> bool {
    true
}
