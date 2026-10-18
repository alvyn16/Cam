//! Restore desktop UI exactly when the recording's guard is released.
use windows::{
    Win32::{
        Foundation::{BOOL, HWND, LPARAM},
        UI::WindowsAndMessaging::*,
    },
    core::w,
};
pub struct DesktopGuard {
    hidden: Vec<isize>,
}
impl DesktopGuard {
    pub fn new(icons: bool, taskbar: bool) -> Self {
        let mut hidden = vec![];
        unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
            unsafe {
                let list = &mut *(param.0 as *mut Vec<isize>);
                if let Ok(view) = FindWindowExW(hwnd, None, w!("SHELLDLL_DefView"), None) {
                    if IsWindowVisible(view).as_bool() {
                        let _ = ShowWindow(view, SW_HIDE);
                        list.push(view.0 as isize);
                    }
                }
            }
            BOOL(1)
        }
        unsafe {
            if icons {
                let _ = EnumWindows(Some(visit), LPARAM(&mut hidden as *mut _ as isize));
            }
            if taskbar {
                for class in [w!("Shell_TrayWnd"), w!("Shell_SecondaryTrayWnd")] {
                    if let Ok(h) = FindWindowW(class, None) {
                        if IsWindowVisible(h).as_bool() {
                            let _ = ShowWindow(h, SW_HIDE);
                            hidden.push(h.0 as isize);
                        }
                    }
                }
            }
        }
        Self { hidden }
    }
}
impl Drop for DesktopGuard {
    fn drop(&mut self) {
        for &h in &self.hidden {
            unsafe {
                let _ = ShowWindow(HWND(h as _), SW_SHOWNA);
            }
        }
    }
}
