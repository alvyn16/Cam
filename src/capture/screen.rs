//! Windows screen and window queries.

use crate::platform::ScreenCaptureBackend;
use crate::platform::windows::WindowsScreenCaptureBackend;
use anyhow::{Result, anyhow, bail};

#[derive(Clone, Debug)]
pub struct DisplayInfo {
    pub id: u32,
    pub points: (i64, i64),
    pub pixels: (usize, usize),
}

#[derive(Clone, Debug)]
pub struct PointOrigin {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug)]
pub struct SizeFrame {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub struct RectFrame {
    pub origin: PointOrigin,
    pub size: SizeFrame,
}

#[derive(Clone, Debug)]
pub struct AppHandle {
    pub pid: i32,
}

#[allow(non_snake_case)]
impl AppHandle {
    pub fn processID(&self) -> i32 {
        self.pid
    }
}

#[derive(Clone, Debug)]
pub struct WindowHandle {
    pub pid: i32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[allow(non_snake_case)]
impl WindowHandle {
    pub fn owningApplication(&self) -> Option<AppHandle> {
        Some(AppHandle { pid: self.pid })
    }

    pub fn frame(&self) -> RectFrame {
        RectFrame {
            origin: PointOrigin {
                x: self.x,
                y: self.y,
            },
            size: SizeFrame {
                width: self.width,
                height: self.height,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub size: (f64, f64),
    pub window: WindowHandle,
    pub display: Option<DisplayInfo>,
}

pub fn displays() -> Result<Vec<DisplayInfo>> {
    let b = WindowsScreenCaptureBackend::new();
    let ds = b.displays()?;
    Ok(ds
        .into_iter()
        .map(|d| DisplayInfo {
            id: d.id,
            points: d.points,
            pixels: d.pixels,
        })
        .collect())
}

pub fn windows() -> Result<Vec<WindowInfo>> {
    let b = WindowsScreenCaptureBackend::new();
    let ws = b.windows()?;
    let ds = displays().unwrap_or_default();
    Ok(ws
        .into_iter()
        .map(|w| {
            use windows::Win32::{
                Foundation::{HWND, RECT},
                Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow},
                UI::WindowsAndMessaging::{GetWindowRect, GetWindowThreadProcessId},
            };
            let hwnd = HWND(w.id as _);
            let mut r = RECT::default();
            let mut pid = 0;
            unsafe {
                let _ = GetWindowRect(hwnd, &mut r);
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }.0 as u32;
            WindowInfo {
                id: w.id,
                app: w.app,
                title: w.title,
                size: w.size,
                window: WindowHandle {
                    pid: pid as i32,
                    x: r.left as f64,
                    y: r.top as f64,
                    width: w.size.0,
                    height: w.size.1,
                },
                display: ds.iter().find(|d| d.id == monitor).cloned(),
            }
        })
        .collect())
}

pub fn find_window(sel: &str) -> Result<WindowInfo> {
    if sel.trim().is_empty() {
        bail!("no window given");
    }
    let ws = windows()?;
    if let Ok(id) = sel.parse::<u32>() {
        if let Some(w) = ws.into_iter().find(|w| w.id == id) {
            return Ok(w);
        }
        bail!("no on-screen window with id {id}");
    }
    let lower = sel.to_lowercase();
    ws.into_iter()
        .filter(|w| {
            format!("{} {}", w.app, w.title)
                .to_lowercase()
                .contains(&lower)
        })
        .max_by(|a, b| (a.size.0 * a.size.1).total_cmp(&(b.size.0 * b.size.1)))
        .ok_or_else(|| anyhow!("no on-screen window matches {sel:?}"))
}

pub enum Target {
    Display(DisplayInfo, Option<crate::engine::geom::Rect>),
    Window(WindowInfo),
}
