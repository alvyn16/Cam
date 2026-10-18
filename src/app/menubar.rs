//! Windows notification-area controls.
use windows::{
    Win32::{
        Foundation::*,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{HSTRING, w},
};
pub struct View<'a> {
    pub recording: bool,
    pub paused: bool,
    pub finishing: bool,
    pub clock: &'a str,
}
pub struct MenuBar {
    hwnd: isize,
    thread: Option<std::thread::JoinHandle<()>>,
    text: String,
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            m if m == WM_APP + 10 => {
                if lparam.0 as u32 == WM_LBUTTONDBLCLK {
                    super::push(super::Cmd::ShowWindow);
                }
                if lparam.0 as u32 == WM_RBUTTONUP {
                    if let Ok(menu) = CreatePopupMenu() {
                        for (id, label) in [
                            (1, "Show Cam"),
                            (2, "Start / stop recording"),
                            (3, "Pause / resume"),
                            (4, "Cancel recording"),
                            (5, "Record area"),
                            (6, "Record window"),
                            (7, "Projects folder"),
                            (8, "Quit"),
                        ] {
                            let _ = AppendMenuW(menu, MF_STRING, id, &HSTRING::from(label));
                        }
                        let mut p = POINT::default();
                        let _ = GetCursorPos(&mut p);
                        let _ = SetForegroundWindow(hwnd);
                        let id = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_RIGHTBUTTON,
                            p.x,
                            p.y,
                            0,
                            hwnd,
                            None,
                        )
                        .0;
                        let cmd = match id {
                            1 => Some(super::Cmd::ShowWindow),
                            2 => Some(super::Cmd::ToggleRecording),
                            3 => Some(super::Cmd::TogglePause),
                            4 => Some(super::Cmd::Cancel),
                            5 => Some(super::Cmd::SelectArea(true)),
                            6 => Some(super::Cmd::SelectWindow(true)),
                            7 => Some(super::Cmd::OpenFolder),
                            8 => Some(super::Cmd::Quit),
                            _ => None,
                        };
                        if let Some(c) = cmd {
                            super::push(c);
                        }
                        let _ = DestroyMenu(menu);
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let mut data = NOTIFYICONDATAW::default();
                data.cbSize = std::mem::size_of_val(&data) as u32;
                data.hWnd = hwnd;
                data.uID = 1;
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
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
impl MenuBar {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || unsafe {
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Cam tray"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            ) else {
                let _ = tx.send(0);
                return;
            };
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, procedure as *const () as isize);
            let mut data = NOTIFYICONDATAW::default();
            data.cbSize = std::mem::size_of_val(&data) as u32;
            data.hWnd = hwnd;
            data.uID = 1;
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            data.uCallbackMessage = WM_APP + 10;
            data.hIcon = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
            data.szTip[..4].copy_from_slice(&[67, 97, 109, 0]);
            let _ = Shell_NotifyIconW(NIM_ADD, &data);
            let _ = tx.send(hwnd.0 as isize);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        Self {
            hwnd: rx.recv().unwrap_or(0),
            thread: Some(thread),
            text: "Cam".into(),
        }
    }
    pub fn set_hotkey(&self, _label: &str, _ok: bool) {}
    pub fn title(&self) -> Option<String> {
        Some(self.text.clone())
    }
    pub fn update(&mut self, v: &View) {
        let text = if v.finishing {
            "Cam — Saving".into()
        } else if v.recording {
            format!(
                "Cam — {} {}",
                if v.paused { "Paused" } else { "Recording" },
                v.clock
            )
        } else {
            "Cam".into()
        };
        if self.text == text {
            return;
        }
        self.text = text;
        let mut data = NOTIFYICONDATAW::default();
        data.cbSize = std::mem::size_of_val(&data) as u32;
        data.hWnd = HWND(self.hwnd as _);
        data.uID = 1;
        data.uFlags = NIF_TIP;
        for (dst, src) in data
            .szTip
            .iter_mut()
            .take(127)
            .zip(self.text.encode_utf16())
        {
            *dst = src;
        }
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }
}
impl Drop for MenuBar {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            unsafe {
                let _ = PostMessageW(HWND(self.hwnd as _), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
