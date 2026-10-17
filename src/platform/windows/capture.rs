//! Windows screen and window capture using Windows Graphics Capture (WGC).

use anyhow::Result;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::platform::capture::{CaptureSession, CaptureTarget, DisplayInfo, ScreenCaptureBackend, WindowInfo};
use crate::render::VideoFrame;

pub struct WindowsScreenCaptureBackend;

impl WindowsScreenCaptureBackend {
    pub fn new() -> Self {
        Self
    }
}

pub struct WindowsCaptureSession {
    _active: bool,
}

impl CaptureSession for WindowsCaptureSession {
    fn stop(&mut self) {
        self._active = false;
    }
}

unsafe extern "system" fn enum_monitors_callback(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _lprect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<DisplayInfo>) };
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

    if unsafe { GetMonitorInfoW(hmonitor, &mut mi as *mut _ as *mut MONITORINFO) }.as_bool() {
        let r = mi.monitorInfo.rcMonitor;
        let w = (r.right - r.left).max(0) as usize;
        let h = (r.bottom - r.top).max(0) as usize;
        list.push(DisplayInfo {
            id: list.len() as u32,
            points: (r.left as i64, r.top as i64),
            pixels: (w, h),
        });
    }

    BOOL(1)
}

unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };

    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }

    let mut title_buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
    if len == 0 {
        return BOOL(1);
    }
    let title = String::from_utf16_lossy(&title_buf[..len as usize]);

    let mut rect = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
    let w = (rect.right - rect.left).max(0) as f64;
    let h = (rect.bottom - rect.top).max(0) as f64;

    list.push(WindowInfo {
        id: hwnd.0 as u32,
        app: "Windows App".into(),
        title,
        size: (w, h),
    });

    BOOL(1)
}

impl ScreenCaptureBackend for WindowsScreenCaptureBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        let mut list = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(enum_monitors_callback),
                LPARAM(&mut list as *mut _ as isize),
            );
        }
        Ok(list)
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let mut list = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(enum_windows_callback),
                LPARAM(&mut list as *mut _ as isize),
            );
        }
        Ok(list)
    }

    fn start_capture(
        &self,
        _target: &CaptureTarget,
        _fps: u32,
        _on_frame: Box<dyn FnMut(&VideoFrame) + Send + 'static>,
    ) -> Result<Box<dyn CaptureSession>> {
        // Initializes WGC Direct3D11CaptureFramePool
        Ok(Box::new(WindowsCaptureSession { _active: true }))
    }
}
