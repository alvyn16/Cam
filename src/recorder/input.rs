//! Input logging while recording: mouse position and buttons (polled, no
//! permission needed), the system cursor image, window bounds for window
//! recordings, and key presses (event tap; needs Input Monitoring).

use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSCursor, NSImage};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use crate::engine::geom::Rect;
use crate::project::recording::{CursorImage, MouseKind, mods};
use crate::util::*;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        mask: u64,
        callback: extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
        info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetFlags(event: *const c_void) -> u64;
    fn CGEventGetIntegerValueField(event: *const c_void, field: u32) -> i64;
    fn CGEventKeyboardGetUnicodeString(event: *const c_void, max: usize, actual: *mut usize, buf: *mut u16);
    fn CGWindowListCopyWindowInfo(option: u32, window: u32) -> *const c_void;
    pub fn CGDisplayBounds(display: u32) -> CGRect;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: *mut c_void, order: isize) -> *mut c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: bool) -> i32;
    static kCFRunLoopCommonModes: *const c_void;
    static kCFRunLoopDefaultMode: *const c_void;
}

const COMBINED_SESSION_STATE: i32 = 0;
const KEYBOARD_EVENT_KEYCODE: u32 = 9;
const EVENT_KEY_DOWN: u32 = 10;
const EVENT_KEY_UP: u32 = 11;
const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const LIST_INCLUDING_WINDOW: u32 = 1 << 3;

#[derive(Clone, Copy, Debug)]
pub struct RawMouse {
    pub host: f64,
    pub x: f64,
    pub y: f64,
    pub kind: MouseKind,
    pub button: u8,
    pub cursor: u32,
}

#[derive(Clone, Debug)]
pub struct RawKey {
    pub host: f64,
    pub code: u16,
    pub key: String,
    pub mods: u8,
    pub down: bool,
}

#[derive(Default)]
pub struct InputLog {
    pub mouse: Vec<RawMouse>,
    pub keys: Vec<RawKey>,
    pub cursors: Vec<CursorImage>,
    /// Window bounds over time (host seconds, global points).
    pub bounds: Vec<(f64, Rect)>,
    /// Whether key logging was available.
    pub keys_enabled: bool,
}

pub struct InputLogger {
    stop: Arc<AtomicBool>,
    poll: Option<JoinHandle<InputLog>>,
    keys: Arc<Mutex<Vec<RawKey>>>,
    tap_thread: Option<JoinHandle<()>>,
    keys_enabled: Arc<AtomicBool>,
}

fn mouse_location() -> CGPoint {
    unsafe {
        let e = CGEventCreate(std::ptr::null());
        if e.is_null() {
            return CGPoint::new(0.0, 0.0);
        }
        let p = CGEventGetLocation(e);
        CFRelease(e);
        p
    }
}

/// Bounds of a window in global points, if it still exists.
pub fn window_bounds(id: u32) -> Option<Rect> {
    unsafe {
        let arr = CGWindowListCopyWindowInfo(LIST_INCLUDING_WINDOW, id);
        if arr.is_null() {
            return None;
        }
        let list: &NSArray<NSDictionary<NSString, objc2::runtime::AnyObject>> = &*(arr as *const _);
        let mut out = None;
        if let Some(info) = list.firstObject()
            && let Some(b) = info.objectForKey(&ns("kCGWindowBounds"))
            && let Ok(b) = b.downcast::<NSDictionary>()
        {
            let b: Retained<NSDictionary<NSString, NSNumber>> = Retained::cast_unchecked(b);
            let get = |k: &str| b.objectForKey(&ns(k)).map_or(0.0, |n| n.doubleValue());
            out = Some(Rect::new(get("X"), get("Y"), get("Width"), get("Height")));
        }
        CFRelease(arr);
        out
    }
}

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

fn tiff_hash(image: &NSImage) -> Option<u64> {
    let tiff = image.TIFFRepresentation()?;
    Some(hash_bytes(&tiff.to_vec()))
}

/// A class cursor by selector name, without panicking when AppKit isn't
/// initialised (the bindings assume non-NULL).
fn class_cursor(name: &str) -> Option<Retained<NSCursor>> {
    use objc2::{ClassType, msg_send};
    let cls = NSCursor::class();
    unsafe {
        match name {
            "arrowCursor" => msg_send![cls, arrowCursor],
            "IBeamCursor" => msg_send![cls, IBeamCursor],
            "pointingHandCursor" => msg_send![cls, pointingHandCursor],
            "closedHandCursor" => msg_send![cls, closedHandCursor],
            "openHandCursor" => msg_send![cls, openHandCursor],
            "crosshairCursor" => msg_send![cls, crosshairCursor],
            "resizeLeftRightCursor" => msg_send![cls, resizeLeftRightCursor],
            "resizeUpDownCursor" => msg_send![cls, resizeUpDownCursor],
            "operationNotAllowedCursor" => msg_send![cls, operationNotAllowedCursor],
            "currentSystemCursor" => msg_send![cls, currentSystemCursor],
            _ => None,
        }
    }
}

/// Hashes of well-known system cursors, to name what we capture.
fn known_cursors() -> HashMap<u64, &'static str> {
    [
        ("arrow", "arrowCursor"),
        ("ibeam", "IBeamCursor"),
        ("pointing-hand", "pointingHandCursor"),
        ("closed-hand", "closedHandCursor"),
        ("open-hand", "openHandCursor"),
        ("crosshair", "crosshairCursor"),
        ("resize-left-right", "resizeLeftRightCursor"),
        ("resize-up-down", "resizeUpDownCursor"),
        ("not-allowed", "operationNotAllowedCursor"),
    ]
    .iter()
    .filter_map(|(n, sel)| Some((tiff_hash(&class_cursor(sel)?.image())?, *n)))
    .collect()
}

/// Save the largest representation of a cursor image as PNG.
fn save_png(image: &NSImage, path: &std::path::Path) -> bool {
    let Some(tiff) = image.TIFFRepresentation() else { return false };
    let reps = NSBitmapImageRep::imageRepsWithData(&tiff);
    let best = reps
        .iter()
        .filter_map(|r| r.downcast::<NSBitmapImageRep>().ok())
        .max_by_key(|r| r.pixelsWide());
    let Some(rep) = best else { return false };
    let Some(png) = (unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }) else {
        return false;
    };
    std::fs::write(path, png.to_vec()).is_ok()
}

struct CursorTracker {
    dir: PathBuf,
    known: HashMap<u64, &'static str>,
    seen: HashMap<u64, u32>,
    images: Vec<CursorImage>,
    current: u32,
}

impl CursorTracker {
    fn sample(&mut self) {
        objc2::rc::autoreleasepool(|_| {
            let Some(cursor) = class_cursor("currentSystemCursor") else { return };
            let image = cursor.image();
            let Some(h) = tiff_hash(&image) else { return };
            if let Some(&id) = self.seen.get(&h) {
                self.current = id;
                return;
            }
            let id = self.images.len() as u32;
            let size = image.size();
            let hot = cursor.hotSpot();
            let file = format!("cursor-{id}.png");
            let saved = save_png(&image, &self.dir.join(&file));
            self.images.push(CursorImage {
                id,
                file: saved.then_some(file),
                hotspot: (hot.x / size.width.max(1.0), hot.y / size.height.max(1.0)),
                size: (size.width, size.height),
                system: self.known.get(&h).map(|s| s.to_string()),
            });
            self.seen.insert(h, id);
            self.current = id;
        });
    }
}

/// Label for a key code (specials as glyphs, else the typed character).
fn key_label(code: u16, event: *const c_void) -> String {
    let special = match code {
        36 | 76 => "⏎",
        48 => "⇥",
        49 => "Space",
        51 => "⌫",
        53 => "⎋",
        117 => "⌦",
        123 => "←",
        124 => "→",
        125 => "↓",
        126 => "↑",
        115 => "Home",
        119 => "End",
        116 => "⇞",
        121 => "⇟",
        122 => "F1",
        120 => "F2",
        99 => "F3",
        118 => "F4",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        100 => "F8",
        101 => "F9",
        109 => "F10",
        103 => "F11",
        111 => "F12",
        _ => "",
    };
    if !special.is_empty() {
        return special.into();
    }
    let mut buf = [0u16; 8];
    let mut n = 0usize;
    unsafe { CGEventKeyboardGetUnicodeString(event, buf.len(), &mut n, buf.as_mut_ptr()) };
    let s = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    s.to_uppercase()
}

fn flags_to_mods(f: u64) -> u8 {
    let mut m = 0;
    if f & 0x0010_0000 != 0 {
        m |= mods::CMD;
    }
    if f & 0x0002_0000 != 0 {
        m |= mods::SHIFT;
    }
    if f & 0x0008_0000 != 0 {
        m |= mods::OPT;
    }
    if f & 0x0004_0000 != 0 {
        m |= mods::CTRL;
    }
    if f & 0x0080_0000 != 0 {
        m |= mods::FN;
    }
    m
}

struct TapCtx {
    keys: Arc<Mutex<Vec<RawKey>>>,
    tap: AtomicPtr<c_void>,
}

extern "C" fn on_key(_proxy: *mut c_void, ty: u32, event: *mut c_void, info: *mut c_void) -> *mut c_void {
    let ctx = unsafe { &*(info as *const TapCtx) };
    if ty == EVENT_TAP_DISABLED_BY_TIMEOUT {
        let tap = ctx.tap.load(Ordering::SeqCst);
        if !tap.is_null() {
            unsafe { CGEventTapEnable(tap, true) };
        }
        return event;
    }
    if ty == EVENT_KEY_DOWN || ty == EVENT_KEY_UP {
        let code = unsafe { CGEventGetIntegerValueField(event, KEYBOARD_EVENT_KEYCODE) } as u16;
        let k = RawKey {
            host: secs(host_now()),
            code,
            key: key_label(code, event),
            mods: flags_to_mods(unsafe { CGEventGetFlags(event) }),
            down: ty == EVENT_KEY_DOWN,
        };
        ctx.keys.lock().unwrap().push(k);
    }
    event
}

impl InputLogger {
    /// Start logging. `window` = window id to track for window recordings.
    pub fn start(cursor_dir: PathBuf, window: Option<u32>, log_keys: bool) -> Self {
        let _ = std::fs::create_dir_all(&cursor_dir);
        let stop = Arc::new(AtomicBool::new(false));
        let keys = Arc::new(Mutex::new(vec![]));
        let keys_enabled = Arc::new(AtomicBool::new(false));
        let tap_thread = log_keys.then(|| {
            let (keys, enabled, stop) = (keys.clone(), keys_enabled.clone(), stop.clone());
            std::thread::spawn(move || unsafe {
                let ctx = Box::into_raw(Box::new(TapCtx { keys, tap: AtomicPtr::new(std::ptr::null_mut()) }));
                let mask = (1u64 << EVENT_KEY_DOWN) | (1u64 << EVENT_KEY_UP);
                // Session tap, head insert, listen-only.
                let tap = CGEventTapCreate(1, 0, 1, mask, on_key, ctx as *mut c_void);
                if tap.is_null() {
                    drop(Box::from_raw(ctx));
                    return;
                }
                (*ctx).tap.store(tap, Ordering::SeqCst);
                enabled.store(true, Ordering::SeqCst);
                let src = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
                let current = CFRunLoopGetCurrent();
                CFRunLoopAddSource(current, src, kCFRunLoopCommonModes);
                CGEventTapEnable(tap, true);
                while !stop.load(Ordering::SeqCst) {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.2, false);
                }
                CGEventTapEnable(tap, false);
                CFRelease(src);
                CFRelease(tap);
                drop(Box::from_raw(ctx));
            })
        });
        let stop2 = stop.clone();
        let poll = std::thread::spawn(move || {
            let mut log = InputLog::default();
            let mut cursors =
                CursorTracker { dir: cursor_dir, known: known_cursors(), seen: HashMap::new(), images: vec![], current: 0 };
            cursors.sample();
            let mut last: Option<(f64, f64)> = None;
            let mut buttons = [false; 2];
            let mut tick: u64 = 0;
            while !stop2.load(Ordering::SeqCst) {
                let now = secs(host_now());
                let p = mouse_location();
                if tick % 6 == 0 {
                    cursors.sample();
                }
                if let Some(w) = window
                    && tick % 12 == 0
                    && let Some(b) = window_bounds(w)
                    && log.bounds.last().is_none_or(|(_, last)| *last != b)
                {
                    log.bounds.push((now, b));
                }
                let pressed = [unsafe { CGEventSourceButtonState(COMBINED_SESSION_STATE, 0) }, unsafe {
                    CGEventSourceButtonState(COMBINED_SESSION_STATE, 1)
                }];
                let ev = |kind, button| RawMouse { host: now, x: p.x, y: p.y, kind, button, cursor: cursors.current };
                for b in 0..2 {
                    if pressed[b] != buttons[b] {
                        log.mouse.push(ev(if pressed[b] { MouseKind::Down } else { MouseKind::Up }, b as u8));
                        buttons[b] = pressed[b];
                    }
                }
                if last != Some((p.x, p.y)) {
                    let kind = if buttons[0] { MouseKind::Drag } else { MouseKind::Move };
                    log.mouse.push(ev(kind, 0));
                    last = Some((p.x, p.y));
                }
                tick += 1;
                std::thread::sleep(Duration::from_micros(8_333));
            }
            log.cursors = cursors.images;
            log
        });
        InputLogger { stop, poll: Some(poll), keys, tap_thread, keys_enabled }
    }

    pub fn stop(mut self) -> InputLog {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.tap_thread.take() {
            let _ = t.join();
        }
        let mut log = self.poll.take().map(|t| t.join().unwrap_or_default()).unwrap_or_default();
        log.keys = std::mem::take(&mut *self.keys.lock().unwrap());
        log.keys_enabled = self.keys_enabled.load(Ordering::SeqCst);
        log
    }
}
