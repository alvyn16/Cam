//! Platform abstraction layer for Cam.

pub mod capture;
pub mod audio;
pub mod media;
pub mod input;
pub mod os;

pub mod windows;

pub use capture::ScreenCaptureBackend;
pub use audio::AudioCaptureBackend;
pub use media::MediaBackend;
pub use input::InputBackend;
pub use os::OsIntegrationBackend;
