//! Windows platform backends.

pub mod capture;
pub mod audio;
pub mod media;
pub mod input;
pub mod os;

pub use capture::WindowsScreenCaptureBackend;
pub use audio::WindowsAudioBackend;
pub use media::WindowsMediaBackend;
pub use input::WindowsInputBackend;
pub use os::WindowsOsBackend;
