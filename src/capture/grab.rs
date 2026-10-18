//! Native desktop/window snapshots. Resources stay on the capture thread.
use crate::engine::geom::Rect;
use anyhow::{Context, Result, ensure};
use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_PRINT},
};

#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}
pub fn snapshot(target: &super::screen::Target) -> Result<Image> {
    let (hwnd, rect) = match target {
        super::screen::Target::Display(d, area) => {
            let (_, x, y, w, h) = crate::app::os::display_frame(d.id);
            let a = area.unwrap_or(Rect::new(0.0, 0.0, w, h));
            (None, Rect::new(x + a.x, y + a.y, a.w, a.h))
        }
        super::screen::Target::Window(w) => (
            Some(HWND(w.id as _)),
            crate::recorder::input::window_bounds(w.id).context("window closed")?,
        ),
    };
    let (width, height) = (rect.w.round() as u32, rect.h.round() as u32);
    ensure!(
        width > 0 && height > 0 && width <= 16384 && height <= 16384,
        "invalid capture region"
    );
    unsafe {
        let screen = GetDC(None);
        ensure!(!screen.is_invalid(), "GetDC failed");
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width as i32, height as i32);
        if dc.is_invalid() || bitmap.is_invalid() {
            let _ = ReleaseDC(None, screen);
            anyhow::bail!("capture allocation failed");
        }
        let old = SelectObject(dc, HGDIOBJ(bitmap.0));
        let painted = hwnd.is_some_and(|h| {
            let mut result = 0;
            SendMessageTimeoutW(
                h,
                WM_PRINT,
                windows::Win32::Foundation::WPARAM(dc.0 as usize),
                windows::Win32::Foundation::LPARAM(0x16),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                250,
                Some(&mut result),
            )
            .0 != 0
        });
        let result = if painted {
            Ok(())
        } else if hwnd.is_some() {
            Err(anyhow::anyhow!(
                "the window did not respond to a paint request"
            ))
        } else {
            BitBlt(
                dc,
                0,
                0,
                width as i32,
                height as i32,
                screen,
                rect.x as i32,
                rect.y as i32,
                SRCCOPY | CAPTUREBLT,
            )
            .map_err(anyhow::Error::from)
        };
        SelectObject(dc, old);
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bgra = vec![0; (width * height * 4) as usize];
        let rows = if result.is_ok() {
            GetDIBits(
                dc,
                bitmap,
                0,
                height,
                Some(bgra.as_mut_ptr() as _),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        let _ = ReleaseDC(None, screen);
        result?;
        ensure!(rows == height as i32, "capture did not return a full frame");
        for p in bgra.chunks_exact_mut(4) {
            p[3] = 255;
        }
        Ok(Image {
            width,
            height,
            bgra,
        })
    }
}
