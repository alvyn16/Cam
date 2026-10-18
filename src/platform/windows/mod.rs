//! Windows platform backends.

pub mod audio;
pub mod capture;
pub mod input;
pub mod media;
pub mod os;

pub use audio::WindowsAudioBackend;
pub use capture::WindowsScreenCaptureBackend;
pub use input::WindowsInputBackend;
pub use media::WindowsMediaBackend;
pub use os::WindowsOsBackend;

/// Keep capture geometry in physical pixels, including callers outside winit.
pub fn ensure_dpi_awareness() {
    use windows::Win32::UI::HiDpi::*;
    static PROCESS: std::sync::Once = std::sync::Once::new();
    PROCESS.call_once(|| unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
