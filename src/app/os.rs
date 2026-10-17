//! Windows platform helpers: explorer integration, clipboard, display queries, and DWM styling.

use std::path::Path;
use anyhow::Result;
use windows::core::HSTRING;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

fn win_hwnd(window: &Window) -> Option<HWND> {
    if let Ok(handle) = window.window_handle() {
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            return Some(HWND(h.hwnd.get() as _));
        }
    }
    None
}

/// Reveal a file in Windows File Explorer.
pub fn reveal(path: &Path) {
    let arg = format!("/select,\"{}\"", path.display());
    let _ = std::process::Command::new("explorer").arg(arg).spawn();
}

/// Open a folder in Windows File Explorer.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}

/// Open a file with its default app.
pub fn open(path: &Path) {
    let path_str = path.to_string_lossy().to_string();
    let op = HSTRING::from("open");
    let file = HSTRING::from(path_str);
    unsafe {
        ShellExecuteW(None, &op, &file, None, None, SW_SHOWNORMAL);
    }
}

/// Put a file on the Windows clipboard.
pub fn copy_file(path: &Path) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(path.display().to_string());
    }
}

/// Move a file or package to the Windows Recycle Bin.
pub fn trash(path: &Path) {
    let path_str = format!("{}\0\0", path.display());
    let wide: Vec<u16> = path_str.encode_utf16().collect();
    let mut op = SHFILEOPSTRUCTW {
        hwnd: HWND(std::ptr::null_mut()),
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(wide.as_ptr()),
        pTo: windows::core::PCWSTR(std::ptr::null()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT).0 as u16,
        fAnyOperationsAborted: BOOL(0),
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: windows::core::PCWSTR(std::ptr::null()),
    };
    unsafe {
        let _ = SHFileOperationW(&mut op);
    }
}

/// Save the current desktop wallpaper to `dest` as PNG.
pub fn save_desktop_wallpaper(dest: &Path) -> Result<()> {
    let mut buf = [0u16; 512];
    unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )?;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let wallpaper_path = String::from_utf16_lossy(&buf[..len]);
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let img = image::open(&wallpaper_path)?;
    img.save(dest)?;
    Ok(())
}

/// IDs of the active displays (main display first).
pub fn display_ids() -> Vec<u32> {
    let mut ids = Vec::new();
    unsafe extern "system" fn callback(hmon: HMONITOR, _: HDC, _: *mut RECT, lparam: LPARAM) -> BOOL {
        let list = unsafe { &mut *(lparam.0 as *mut Vec<u32>) };
        list.push(hmon.0 as u32);
        BOOL(1)
    }
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(callback), LPARAM(&mut ids as *mut _ as isize));
    }
    if ids.is_empty() {
        vec![0]
    } else {
        ids
    }
}

/// The display under the mouse pointer.
pub fn display_under_pointer() -> u32 {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        hmon.0 as u32
    }
}

/// A display's frame in global points (top-left origin). (id, x, y, w, h)
pub fn display_frame(id: u32) -> (u32, f64, f64, f64, f64) {
    let hmon = if id == 0 {
        unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) }
    } else {
        HMONITOR(id as _)
    };

    let mut mi = MONITORINFO::default();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(hmon, &mut mi) }.as_bool() {
        let r = mi.rcMonitor;
        let w = (r.right - r.left) as f64;
        let h = (r.bottom - r.top) as f64;
        (id, r.left as f64, r.top as f64, w, h)
    } else {
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) } as f64;
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) } as f64;
        (0, 0.0, 0.0, w, h)
    }
}

/// The main display's ID (0).
pub fn main_display() -> u32 {
    0
}

/// Window HWND identifier.
pub fn window_number(window: &Window) -> Option<isize> {
    win_hwnd(window).map(|h| h.0 as isize)
}

/// Whether the window is in full screen.
pub fn is_fullscreen(window: &Window) -> bool {
    window.fullscreen().is_some()
}

/// The display the window is full screen on.
pub fn fullscreen_display(_window: &Window) -> Option<u32> {
    None
}

/// Inset traffic lights (macOS specific, no-op on Windows).
pub fn inset_traffic_lights(_window: &Window, _x: f64, _bar_h: f64) {}

/// Bring the app to the front.
pub fn activate() {}

/// Open URLs handler (macOS specific, no-op on Windows).
pub fn install_open_handler() {}
