//! Capture cursor shape changes, including the native hotspot and alpha bitmap.
use crate::project::recording::CursorImage;
use std::path::Path;
use windows::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};
pub fn current(dir: &Path, id: u32) -> Option<(usize, CursorImage)> {
    unsafe {
        let mut cursor = CURSORINFO::default();
        cursor.cbSize = std::mem::size_of_val(&cursor) as u32;
        GetCursorInfo(&mut cursor).ok()?;
        let mut icon = ICONINFO::default();
        GetIconInfo(cursor.hCursor, &mut icon).ok()?;
        let mut bitmap = BITMAP::default();
        let color = !icon.hbmColor.is_invalid();
        let image = if color
            && GetObjectW(
                icon.hbmColor,
                std::mem::size_of_val(&bitmap) as i32,
                Some(&mut bitmap as *mut _ as _),
            ) > 0
        {
            let (w, h) = (bitmap.bmWidth.max(1) as u32, bitmap.bmHeight.max(1) as u32);
            let dc = GetDC(None);
            let mut info = BITMAPINFO::default();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            };
            let mut pixels = vec![0; (w * h * 4) as usize];
            let rows = GetDIBits(
                dc,
                icon.hbmColor,
                0,
                h,
                Some(pixels.as_mut_ptr() as _),
                &mut info,
                DIB_RGB_COLORS,
            );
            let _ = ReleaseDC(None, dc);
            if rows > 0 && pixels.chunks_exact(4).any(|p| p[3] != 0) {
                for p in pixels.chunks_exact_mut(4) {
                    p.swap(0, 2);
                    let a = p[3] as u32;
                    if a > 0 {
                        for v in &mut p[..3] {
                            *v = ((*v as u32 * 255 / a).min(255)) as u8;
                        }
                    }
                }
                let _ = std::fs::create_dir_all(dir);
                let name = format!("cursor-{id}.png");
                let saved = image::save_buffer(
                    dir.join(&name),
                    &pixels,
                    w,
                    h,
                    image::ExtendedColorType::Rgba8,
                )
                .is_ok();
                Some((w, h, saved.then_some(format!("cursors/{name}"))))
            } else {
                None
            }
        } else {
            None
        };
        if !icon.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(icon.hbmColor.0));
        }
        if !icon.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(icon.hbmMask.0));
        }
        let (w, h, file) = image.unwrap_or((32, 32, None));
        let system = if LoadCursorW(None, IDC_IBEAM).ok() == Some(cursor.hCursor) {
            "ibeam"
        } else {
            "arrow"
        };
        Some((
            cursor.hCursor.0 as usize,
            CursorImage {
                id,
                file,
                hotspot: (
                    icon.xHotspot as f64 / w as f64,
                    icon.yHotspot as f64 / h as f64,
                ),
                size: (w as f64, h as f64),
                system: Some(system.into()),
            },
        ))
    }
}
