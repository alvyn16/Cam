//! Native non-activating recording controls on their own message thread.
use windows::{
    Win32::{Foundation::*, UI::WindowsAndMessaging::*},
    core::{HSTRING, w},
};
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}
#[derive(Clone, PartialEq)]
pub enum State {
    Countdown(u64),
    Recording { clock: String, paused: bool },
}
pub struct RecordBar {
    hwnd: isize,
    label: isize,
    pause: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                match wparam.0 & 0xffff {
                    1 => super::push(super::Cmd::ToggleRecording),
                    2 => super::push(super::Cmd::TogglePause),
                    3 => super::push(super::Cmd::Restart),
                    4 => super::push(super::Cmd::Cancel),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTCAPTION as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_CLOSE => {
                super::push(super::Cmd::Cancel);
                LRESULT(0)
            }
            m if m == WM_APP + 1 => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
impl RecordBar {
    pub fn new(display: u32, origin: Option<Point>, _stop_hint: &str, _pause_hint: &str) -> Self {
        let (_, x, y, w, h) = super::os::display_frame(display);
        let origin = origin.unwrap_or(Point {
            x: x + w / 2.0 - 230.0,
            y: y + h - 110.0,
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || unsafe {
            let created = (|| -> windows::core::Result<(HWND, HWND, HWND)> {
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("Cam recording"),
                    WS_POPUP | WS_BORDER | WS_VISIBLE,
                    origin.x as i32,
                    origin.y as i32,
                    460,
                    48,
                    None,
                    None,
                    None,
                    None,
                )?;
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, procedure as *const () as isize);
                let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                let label = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("Starting..."),
                    WS_CHILD | WS_VISIBLE,
                    10,
                    15,
                    140,
                    25,
                    hwnd,
                    None,
                    None,
                    None,
                )?;
                let mut pause = HWND::default();
                for (id, text, x, width) in [
                    (2, "Pause", 150, 65),
                    (3, "Restart", 219, 70),
                    (4, "Cancel", 293, 70),
                    (1, "Stop", 367, 80),
                ] {
                    let h = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("BUTTON"),
                        &HSTRING::from(text),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                        x,
                        8,
                        width,
                        32,
                        hwnd,
                        HMENU(id as _),
                        None,
                        None,
                    )?;
                    if id == 2 {
                        pause = h;
                    }
                }
                Ok((hwnd, label, pause))
            })();
            let Ok((hwnd, label, pause)) = created else {
                let _ = tx.send((0, 0, 0));
                return;
            };
            let _ = tx.send((hwnd.0 as isize, label.0 as isize, pause.0 as isize));
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        let (hwnd, label, pause) = rx.recv().unwrap_or_default();
        Self {
            hwnd,
            label,
            pause,
            thread: Some(thread),
        }
    }
    pub fn update(&mut self, state: State) {
        let (text, paused) = match state {
            State::Countdown(n) => (format!("Starts in {n}..."), false),
            State::Recording { clock, paused } => (
                format!("{} {clock}", if paused { "Paused" } else { "Recording" }),
                paused,
            ),
        };
        unsafe {
            let _ = SetWindowTextW(HWND(self.label as _), &HSTRING::from(text));
            let _ = SetWindowTextW(
                HWND(self.pause as _),
                if paused { w!("Resume") } else { w!("Pause") },
            );
        }
    }
    pub fn origin(&self) -> Point {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(HWND(self.hwnd as _), &mut r);
        }
        Point {
            x: r.left as f64,
            y: r.top as f64,
        }
    }
    pub fn window_number(&self) -> isize {
        self.hwnd
    }
    pub fn draggable(&self) -> bool {
        self.hwnd != 0
    }
}
impl Drop for RecordBar {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            unsafe {
                let _ = PostMessageW(HWND(self.hwnd as _), WM_APP + 1, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
