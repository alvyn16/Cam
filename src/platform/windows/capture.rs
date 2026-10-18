//! Windows display/window enumeration and native frame callbacks.

use anyhow::Result;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::platform::capture::{
    CaptureSession, CaptureTarget, DisplayInfo, ScreenCaptureBackend, WindowInfo,
};
use crate::render::VideoFrame;

pub struct WindowsScreenCaptureBackend;

impl WindowsScreenCaptureBackend {
    pub fn new() -> Self {
        Self
    }
}

pub struct WindowsCaptureSession {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl CaptureSession for WindowsCaptureSession {
    fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
impl Drop for WindowsCaptureSession {
    fn drop(&mut self) {
        self.stop();
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
            id: hmonitor.0 as u32,
            points: (w as i64, h as i64),
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
        app: {
            use windows::Win32::System::Threading::*;
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            let mut name = format!("Process {pid}");
            unsafe {
                if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let mut buf = [0u16; 1024];
                    let mut size = buf.len() as u32;
                    if QueryFullProcessImageNameW(
                        process,
                        PROCESS_NAME_WIN32,
                        windows::core::PWSTR(buf.as_mut_ptr()),
                        &mut size,
                    )
                    .is_ok()
                    {
                        let path = String::from_utf16_lossy(&buf[..size as usize]);
                        name = std::path::Path::new(&path)
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                    }
                    let _ = CloseHandle(process);
                }
            }
            name
        },
        title,
        size: (w, h),
    });

    BOOL(1)
}

impl ScreenCaptureBackend for WindowsScreenCaptureBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        super::ensure_dpi_awareness();
        let mut list: Vec<DisplayInfo> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(enum_monitors_callback),
                LPARAM(&mut list as *mut _ as isize),
            );
        }
        let primary =
            unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) }.0 as u32;
        list.sort_by_key(|d| d.id != primary);
        Ok(list)
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        super::ensure_dpi_awareness();
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
        target: &CaptureTarget,
        fps: u32,
        mut on_frame: Box<dyn FnMut(&VideoFrame) + Send + 'static>,
    ) -> Result<Box<dyn CaptureSession>> {
        let target = match target {
            CaptureTarget::Display { id } | CaptureTarget::Area { display: id, .. } => {
                let d = crate::capture::screen::displays()?
                    .into_iter()
                    .find(|d| *id == 0 || d.id == *id)
                    .ok_or_else(|| anyhow::anyhow!("display unavailable"))?;
                let area = if let CaptureTarget::Area { rect, .. } = target {
                    Some(*rect)
                } else {
                    None
                };
                Some(crate::capture::screen::Target::Display(d, area))
            }
            CaptureTarget::Window { id } => Some(crate::capture::screen::Target::Window(
                crate::capture::screen::find_window(&id.to_string())?,
            )),
            CaptureTarget::Pattern => None,
            CaptureTarget::Device { .. } => anyhow::bail!("device capture uses the camera backend"),
        };
        let capture = target
            .as_ref()
            .map(|t| crate::capture::wgc::Stream::start(t, fps, false))
            .transpose()?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let end = stop.clone();
        let thread = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            while !end.load(std::sync::atomic::Ordering::Acquire) {
                let now = std::time::Instant::now();
                let image = match &capture {
                    Some(c) => c.snapshot(),
                    None => Ok(crate::capture::grab::Image {
                        width: 640,
                        height: 400,
                        bgra: crate::synth::screen_bgra(
                            640,
                            400,
                            start.elapsed().as_secs_f64() * 1000.0,
                        ),
                    }),
                };
                match image {
                    Ok(f) => on_frame(&VideoFrame {
                        width: f.width,
                        height: f.height,
                        stride: (f.width * 4) as usize,
                        bgra: &f.bgra,
                    }),
                    Err(e) => {
                        eprintln!("capture stopped: {e:#}");
                        break;
                    }
                }
                std::thread::sleep(
                    std::time::Duration::from_secs_f64(1.0 / fps.max(1) as f64)
                        .saturating_sub(now.elapsed()),
                );
            }
        });
        Ok(Box::new(WindowsCaptureSession {
            stop,
            thread: Some(thread),
        }))
    }
}
