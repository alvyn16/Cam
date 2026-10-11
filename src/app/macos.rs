//! Window chrome helpers shared with paper-rs: traffic lights inset into the
//! unified toolbar of a transparent-titlebar window.

use objc2_app_kit::{NSView, NSWindow, NSWindowButton};
use objc2_foundation::NSPoint;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

fn ns_window(window: &Window) -> Option<objc2::rc::Retained<NSWindow>> {
    let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: winit hands out a valid NSView pointer for the lifetime of the window, and we
    // are on the main thread (all winit window callbacks are).
    let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
    view.window()
}


/// Move the close/minimise/zoom buttons so they start `x` points from the left and are
/// vertically centred in a titlebar area `bar_h` points tall (our toolbar). AppKit lays the
/// titlebar out again on show, resize and fullscreen changes, so this is re-applied from
/// redraw; it returns early when the buttons are already in place.
pub fn inset_traffic_lights(window: &Window, x: f64, bar_h: f64) {
    let Some(win) = ns_window(window) else { return };
    let (Some(close), Some(mini), Some(zoom)) = (
        win.standardWindowButton(NSWindowButton::CloseButton),
        win.standardWindowButton(NSWindowButton::MiniaturizeButton),
        win.standardWindowButton(NSWindowButton::ZoomButton),
    ) else {
        return;
    };
    // SAFETY: plain view-hierarchy getters on main-thread AppKit objects.
    let Some(container) = (unsafe { close.superview().and_then(|v| v.superview()) }) else {
        return;
    };
    let close_rect = close.frame();
    let btn_y = ((bar_h - close_rect.size.height) / 2.0).round();
    if (close_rect.origin.x - x).abs() < 0.5
        && (close_rect.origin.y - btn_y).abs() < 0.5
        && (container.frame().size.height - bar_h).abs() < 0.5
    {
        return;
    }
    let mut bar = container.frame();
    bar.size.height = bar_h;
    bar.origin.y = win.frame().size.height - bar_h;
    container.setFrame(bar);
    // The buttons' own superview (NSTitlebarView) must grow with the container.
    // SAFETY: as above.
    if let Some(tv) = unsafe { close.superview() } {
        let mut f = tv.frame();
        f.origin.y = 0.0;
        f.size.height = bar_h;
        tv.setFrame(f);
    }
    let gap = mini.frame().origin.x - close_rect.origin.x;
    for (i, b) in [close, mini, zoom].iter().enumerate() {
        b.setFrameOrigin(NSPoint::new(x + gap * i as f64, btn_y));
    }
}


/// Bring the app (an accessory app without a Dock icon) to the front.
pub fn activate() {
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(mtm).activate();
    }
}

/// Reveal a file in Finder.
pub fn reveal(path: &std::path::Path) {
    use objc2_foundation::{NSArray, NSURL};
    let url = NSURL::fileURLWithPath(&crate::util::ns(&path.to_string_lossy()));
    objc2_app_kit::NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
}

/// Open a folder in Finder.
pub fn open_folder(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let url = objc2_foundation::NSURL::fileURLWithPath(&crate::util::ns(&dir.to_string_lossy()));
    objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url);
}

/// Open a file with its default app.
pub fn open(path: &std::path::Path) {
    let url = objc2_foundation::NSURL::fileURLWithPath(&crate::util::ns(&path.to_string_lossy()));
    objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url);
}

/// Put a file on the pasteboard (paste into Slack, Mail, Finder…).
pub fn copy_file(path: &std::path::Path) {
    use objc2::runtime::ProtocolObject;
    use objc2_foundation::{NSArray, NSURL};
    let url = NSURL::fileURLWithPath(&crate::util::ns(&path.to_string_lossy()));
    let pb = objc2_app_kit::NSPasteboard::generalPasteboard();
    pb.clearContents();
    let item: objc2::rc::Retained<ProtocolObject<dyn objc2_app_kit::NSPasteboardWriting>> = ProtocolObject::from_retained(url);
    pb.writeObjects(&NSArray::from_retained_slice(&[item]));
}

/// Move a file or package to the Trash.
pub fn trash(path: &std::path::Path) {
    let url = objc2_foundation::NSURL::fileURLWithPath(&crate::util::ns(&path.to_string_lossy()));
    let fm = objc2_foundation::NSFileManager::defaultManager();
    if let Err(e) = fm.trashItemAtURL_resultingItemURL_error(&url, None) {
        eprintln!("glide: couldn't trash {}: {}", path.display(), e.localizedDescription());
    }
}

/// Save the main display's current desktop picture as a PNG at `dest`
/// (desktop pictures are often HEIC, which the renderer doesn't decode).
pub fn save_desktop_wallpaper(dest: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::anyhow;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSScreen, NSWorkspace};
    let mtm = objc2::MainThreadMarker::new().ok_or_else(|| anyhow!("not on the main thread"))?;
    let screen = NSScreen::mainScreen(mtm).ok_or_else(|| anyhow!("no screen"))?;
    let url = NSWorkspace::sharedWorkspace()
        .desktopImageURLForScreen(&screen)
        .ok_or_else(|| anyhow!("macOS didn't report a desktop picture"))?;
    let image = NSImage::initWithContentsOfURL(NSImage::alloc(), &url)
        .ok_or_else(|| anyhow!("the desktop picture isn't an image (dynamic or video wallpapers aren't supported)"))?;
    let tiff = image.TIFFRepresentation().ok_or_else(|| anyhow!("couldn't read the desktop picture"))?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or_else(|| anyhow!("couldn't decode the desktop picture"))?;
    let props = objc2_foundation::NSDictionary::new();
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        .ok_or_else(|| anyhow!("couldn't encode the desktop picture"))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, png.to_vec())?;
    Ok(())
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> objc2_core_foundation::CGRect;
}

/// A display's frame in global points (top-left origin of the main display);
/// id 0 means the main display. Returns (id, x, y, w, h).
pub fn display_frame(id: u32) -> (u32, f64, f64, f64, f64) {
    // SAFETY: plain CoreGraphics queries.
    unsafe {
        let id = if id == 0 { CGMainDisplayID() } else { id };
        let r = CGDisplayBounds(id);
        (id, r.origin.x, r.origin.y, r.size.width, r.size.height)
    }
}

/// The window's AppKit window number (CGWindowID), for screencapture.
pub fn window_number(window: &Window) -> Option<isize> {
    ns_window(window).map(|w| w.windowNumber())
}
