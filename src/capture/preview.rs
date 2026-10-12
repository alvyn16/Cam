//! Live preview of a capture source for the recording-setup canvas: a
//! separate, low-rate ScreenCaptureKit stream (small frames, a few per
//! second) that only keeps the newest frame, copied out as RGBA.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::*;
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::*;
use dispatch2::DispatchQueue;

use super::screen;

/// What to preview (display ids as CoreGraphics reports them; 0 = main).
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Display { id: u32 },
    /// A region of a display in points (x, y, w, h, top-left origin).
    Area { display: u32, rect: [f64; 4] },
    Window { id: u32 },
}

/// One preview frame, RGBA.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

type Callback = Box<dyn Fn() + Send + Sync>;

pub struct Ivars {
    latest: Arc<Mutex<Option<Frame>>>,
    on_frame: Callback,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "GlidePreviewOutput"]
    #[ivars = Ivars]
    pub struct PreviewOutput;

    unsafe impl NSObjectProtocol for PreviewOutput {}

    unsafe impl SCStreamOutput for PreviewOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output(&self, _stream: &SCStream, sb: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Screen {
                self.handle(sb);
            }
        }
    }
);

impl PreviewOutput {
    fn create(latest: Arc<Mutex<Option<Frame>>>, on_frame: Callback) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { latest, on_frame });
        unsafe { msg_send![super(this), init] }
    }

    fn handle(&self, sb: &CMSampleBuffer) {
        // SAFETY: a valid sample buffer from ScreenCaptureKit; the pixel
        // buffer is locked read-only while copied.
        let Some(pb) = (unsafe { sb.is_valid().then(|| sb.image_buffer()).flatten() }) else { return };
        let frame = unsafe {
            CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
            let (w, h) = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb));
            let stride = CVPixelBufferGetBytesPerRow(&pb);
            let base = CVPixelBufferGetBaseAddress(&pb) as *const u8;
            let frame = (!base.is_null() && w > 0 && h > 0).then(|| {
                let src = std::slice::from_raw_parts(base, stride * h);
                let mut rgba = Vec::with_capacity(w * h * 4);
                for row in src.chunks(stride).take(h) {
                    for px in row[..w * 4].chunks_exact(4) {
                        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    }
                }
                Frame { width: w as u32, height: h as u32, rgba }
            });
            CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
            frame
        };
        if let Some(f) = frame {
            *self.ivars().latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
            (self.ivars().on_frame)();
        }
    }
}

/// A running preview stream; stops when dropped.
pub struct LivePreview {
    stream: Retained<SCStream>,
    _output: Retained<PreviewOutput>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
    latest: Arc<Mutex<Option<Frame>>>,
}

// SAFETY: the stream is only started and stopped through thread-safe
// ScreenCaptureKit calls; frames arrive on its own queue.
unsafe impl Send for LivePreview {}

impl LivePreview {
    /// Start previewing `source` at most `max_px` on the long edge and `fps`
    /// frames per second, calling `on_frame` (from a capture queue) after each
    /// new frame. Glide's own windows are left out.
    pub fn start(source: &Source, max_px: u32, fps: u32, on_frame: impl Fn() + Send + Sync + 'static) -> Result<LivePreview> {
        let displays = screen::displays()?;
        let display_for = |id: u32| displays.iter().find(|d| d.id == id || id == 0).context("display not found");
        // SAFETY: ScreenCaptureKit configuration on objects we own.
        unsafe {
            let conf = SCStreamConfiguration::new();
            let (filter, size) = match source {
                Source::Display { id } | Source::Area { display: id, .. } => {
                    let d = display_for(*id)?;
                    let me = std::process::id() as i32;
                    let mine: Vec<Retained<SCWindow>> = screen::windows()?
                        .into_iter()
                        .filter(|w| w.window.owningApplication().is_some_and(|a| a.processID() == me))
                        .map(|w| w.window)
                        .collect();
                    let filter = SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &d.display, &NSArray::from_retained_slice(&mine));
                    let size = match source {
                        Source::Area { rect, .. } => {
                            conf.setSourceRect(CGRect::new(CGPoint::new(rect[0], rect[1]), CGSize::new(rect[2], rect[3])));
                            (rect[2], rect[3])
                        }
                        _ => (d.points.0 as f64, d.points.1 as f64),
                    };
                    (filter, size)
                }
                Source::Window { id } => {
                    let w = screen::windows()?.into_iter().find(|w| w.id == *id).context("that window is no longer on screen")?;
                    (SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &w.window), w.size)
                }
            };
            if size.0 < 2.0 || size.1 < 2.0 {
                bail!("nothing to preview");
            }
            let scale = (max_px as f64 / size.0.max(size.1)).min(2.0);
            let even = |v: f64| ((v * scale).round() as usize / 2 * 2).max(2);
            conf.setWidth(even(size.0));
            conf.setHeight(even(size.1));
            conf.setMinimumFrameInterval(CMTime::new(1, fps.max(1) as i32));
            conf.setPixelFormat(kCVPixelFormatType_32BGRA);
            conf.setQueueDepth(3);
            conf.setShowsCursor(true);
            conf.setScalesToFit(true);
            conf.setPreservesAspectRatio(true);
            conf.setCapturesAudio(false);

            let latest = Arc::new(Mutex::new(None));
            let output = PreviewOutput::create(latest.clone(), Box::new(on_frame));
            let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &conf, None);
            let queue = DispatchQueue::new("glide.preview", None);
            let proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Screen, Some(&queue))
                .map_err(|e| anyhow::anyhow!("preview output: {}", e.localizedDescription()))?;
            wait(|b| stream.startCaptureWithCompletionHandler(Some(b))).context("starting the preview")?;
            Ok(LivePreview { stream, _output: output, _queue: queue, latest })
        }
    }

    /// The newest frame since the last call.
    pub fn take_frame(&self) -> Option<Frame> {
        self.latest.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl Drop for LivePreview {
    fn drop(&mut self) {
        // SAFETY: stopping a stream we started; errors (already stopped) are fine.
        let _ = wait(|b| unsafe { self.stream.stopCaptureWithCompletionHandler(Some(b)) });
    }
}

fn wait(f: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>)) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let block = block2::RcBlock::new(move |err: *mut NSError| {
        // SAFETY: ScreenCaptureKit passes a valid error or null.
        let r = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = tx.send(r);
    });
    f(&block);
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(None) => Ok(()),
        Ok(Some(e)) => bail!("{e}"),
        Err(_) => bail!("timed out"),
    }
}
