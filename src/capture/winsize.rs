//! Window resizing and presets on Windows.

use anyhow::Result;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, SWP_NOMOVE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos,
};

pub const PRESETS: &[(&str, &[(u32, u32)])] = &[
    ("16:9", &[(1280, 720), (1920, 1080), (2560, 1440)]),
    (
        "16:10",
        &[(1280, 800), (1440, 900), (1680, 1050), (1920, 1200)],
    ),
    ("4:3", &[(800, 600), (1024, 768), (1280, 960), (1600, 1200)]),
    ("9:16", &[(720, 1280), (900, 1600), (1080, 1920)]),
    ("Square", &[(800, 800), (1080, 1080), (1280, 1280)]),
];

pub fn trusted(_prompt: bool) -> bool {
    true
}

pub fn resize(id: u32, w: f64, h: f64) -> Result<(f64, f64)> {
    unsafe {
        let hwnd = HWND(id as _);
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            w as i32,
            h as i32,
            SWP_NOMOVE | SWP_NOZORDER,
        )?;
    }
    window_size(id)
}

pub fn raise(id: u32) -> Result<()> {
    unsafe {
        let hwnd = HWND(id as _);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

pub fn window_size(id: u32) -> Result<(f64, f64)> {
    let mut rect = RECT::default();
    unsafe {
        let hwnd = HWND(id as _);
        GetWindowRect(hwnd, &mut rect)?;
    }
    Ok((
        (rect.right - rect.left) as f64,
        (rect.bottom - rect.top) as f64,
    ))
}

pub fn window_of_pid(pid: i32) -> Option<u32> {
    crate::capture::screen::windows()
        .ok()?
        .into_iter()
        .find(|w| w.window.pid == pid)
        .map(|w| w.id)
}
pub fn display_sizes(ws: &[crate::capture::screen::WindowInfo]) -> Vec<(f64, f64)> {
    ws.iter()
        .map(|w| {
            w.display
                .as_ref()
                .map_or((0.0, 0.0), |d| (d.points.0 as f64, d.points.1 as f64))
        })
        .collect()
}
pub fn front_to_back() -> Vec<u32> {
    crate::capture::screen::windows()
        .unwrap_or_default()
        .into_iter()
        .map(|w| w.id)
        .collect()
}
