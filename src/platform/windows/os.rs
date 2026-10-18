//! Windows OS integration: window styling, DWM dark mode/mica, and pill window setup.

use anyhow::Result;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongW, SetWindowLongW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::platform::os::OsIntegrationBackend;

pub struct WindowsOsBackend;

impl WindowsOsBackend {
    pub fn new() -> Self {
        Self
    }

    fn hwnd(window: &Window) -> Option<HWND> {
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                return Some(HWND(h.hwnd.get() as _));
            }
        }
        None
    }

    pub fn style_window(window: &Window) {
        if let Some(hwnd) = Self::hwnd(window) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowDisplayAffinity(
                    hwnd,
                    windows::Win32::UI::WindowsAndMessaging::WDA_EXCLUDEFROMCAPTURE,
                );
                // Enable Immersive Dark Mode for title bar
                let dark_mode: i32 = 1;
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE,
                    &dark_mode as *const _ as _,
                    std::mem::size_of::<i32>() as u32,
                );

                // Try Mica backdrop on Windows 11 (type 2 = Mica, 3 = Acrylic)
                let mica: i32 = 2;
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    &mica as *const _ as _,
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
    }
}

impl OsIntegrationBackend for WindowsOsBackend {
    fn style_window(&self, window: &Window) {
        Self::style_window(window);
    }

    fn style_floating_pill(&self, window: &Window) {
        if let Some(hwnd) = Self::hwnd(window) {
            unsafe {
                let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
                let _ = SetWindowLongW(
                    hwnd,
                    GWL_EXSTYLE,
                    ex | (WS_EX_TOPMOST.0 as i32) | (WS_EX_TOOLWINDOW.0 as i32),
                );
            }
        }
    }

    fn activate_app(&self) {
        crate::app::windows::activate();
    }

    fn is_fullscreen(&self, window: &Window) -> bool {
        crate::app::windows::is_fullscreen(window)
    }

    fn ensure_screen_permission(&self) -> Result<bool> {
        Ok(true)
    }
}
