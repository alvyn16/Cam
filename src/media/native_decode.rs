//! Persistent Media Foundation preview decoder. Decode/scale stays on D3D11;
//! only the reduced preview image is mapped for the existing compositor upload.
mod callback;
mod color;
mod downsample;
#[cfg(test)]
mod performance_tests;
mod scale;
use super::{Frame, mf_encoder::Runtime, preview_decode::Track};
use anyhow::{Context, Result, bail, ensure};
use std::{os::windows::ffi::OsStrExt, sync::Arc};
use windows62::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE, Direct3D10::ID3D10Multithread, Direct3D11::*,
        },
        Media::MediaFoundation::*,
        System::{Com::StructuredStorage::*, Variant::VT_I8},
    },
    core::{GUID, Interface, PCWSTR},
};

pub(super) struct Reader {
    reader: IMFSourceReader,
    _manager: IMFDXGIDeviceManager,
    scaler: scale::Scale,
    track: Track,
    size: (u32, u32),
    current: Option<Arc<Frame>>,
    completion: Arc<callback::Completion>,
    poisoned: bool,
    _runtime: Runtime,
}
impl Reader {
    pub fn open(track: &Track) -> Result<Self> {
        ensure!(
            track.width > 0
                && track.height > 0
                && track.width <= 16384
                && track.height <= 16384
                && track.fps.is_finite()
                && track.fps > 0.0,
            "invalid native preview track"
        );
        let runtime = Runtime::new()?;
        let (mut device, mut context) = (None, None);
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device = device.context("preview D3D11 device")?;
        let context = context.context("preview D3D11 context")?;
        unsafe {
            let _ = context
                .cast::<ID3D10Multithread>()?
                .SetMultithreadProtected(true);
        }
        let (mut token, mut manager) = (0, None);
        unsafe {
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        }
        let manager = manager.context("preview DXGI manager")?;
        unsafe {
            manager.ResetDevice(&device, token)?;
        }
        let mut attributes = None;
        unsafe {
            MFCreateAttributes(&mut attributes, 4)?;
        }
        let attributes = attributes.context("preview attributes")?;
        let (callback, completion) = callback::callback();
        unsafe {
            attributes.SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &callback)?;
            attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &manager)?;
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        }
        let mut path: Vec<u16> = track.path.as_os_str().encode_wide().collect();
        ensure!(!path.contains(&0), "preview path contains NUL");
        path.push(0);
        let reader = unsafe { MFCreateSourceReaderFromURL(PCWSTR(path.as_ptr()), &attributes)? };
        let scale = (1280.0 / track.width.max(track.height) as f64).min(1.0);
        let size = (
            ((track.width as f64 * scale).floor() as u32 / 2 * 2).max(2),
            ((track.height as f64 * scale).floor() as u32 / 2 * 2).max(2),
        );
        unsafe {
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)?;
            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            output.SetUINT64(
                &MF_MT_FRAME_SIZE,
                ((track.width as u64) << 32) | track.height as u64,
            )?;
            reader.SetCurrentMediaType(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                None,
                &output,
            )?;
        }
        let source =
            unsafe { reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0)? };
        // The Windows byte-stream handler can omit encoded color tags even
        // when the source contains them. Probe once per reader, never per frame.
        let probe = super::ffmpeg::probe(&track.path)?;
        let stream = probe["streams"]
            .as_array()
            .and_then(|streams| streams.iter().find(|s| s["codec_type"] == "video"))
            .context("preview color metadata")?;
        let (matrix, full_range) = color::description(
            stream,
            unsafe { source.GetUINT32(&MF_MT_YUV_MATRIX) }.unwrap_or(2),
            unsafe { source.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE) }.unwrap_or(2) == 1,
        )?;
        let scaler = scale::Scale::new(
            &device,
            &context,
            (track.width, track.height),
            size,
            matrix,
            full_range,
        )?;
        Ok(Self {
            reader,
            _manager: manager,
            scaler,
            track: track.clone(),
            size,
            current: None,
            completion,
            poisoned: false,
            _runtime: runtime,
        })
    }
    fn seek(&mut self, at: f64) -> Result<()> {
        let position = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_I8,
                    Anonymous: PROPVARIANT_0_0_0 {
                        hVal: (at * 10_000.0).round() as i64,
                    },
                    ..Default::default()
                }),
            },
        };
        unsafe {
            self.reader.SetCurrentPosition(&GUID::zeroed(), &position)?;
        }
        self.current = None;
        Ok(())
    }
    pub fn frame_at(
        &mut self,
        at: f64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<Arc<Frame>>> {
        ensure!(
            !self.poisoned,
            "native preview flush did not finish; use FFmpeg"
        );
        let at = at.clamp(
            0.0,
            (self.track.duration_ms - 1000.0 / self.track.fps).max(0.0),
        );
        let half = 500.0 / self.track.fps;
        if self
            .current
            .as_ref()
            .is_some_and(|frame| (frame.pts_ms - at).abs() <= half)
        {
            return Ok(self.current.clone());
        }
        let jump = self
            .current
            .as_ref()
            .is_none_or(|frame| at + half < frame.pts_ms || at > frame.pts_ms + 250.0);
        if jump {
            self.seek(at)?;
        }
        let started = std::time::Instant::now();
        for _ in 0..4096 {
            ensure!(!cancelled(), "native preview seek superseded");
            ensure!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "native preview decode timed out"
            );
            self.completion.begin();
            unsafe {
                self.reader.ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    None,
                    None,
                    None,
                )?;
            }
            let read = match self
                .completion
                .wait(cancelled, std::time::Duration::from_secs(5))
            {
                Ok(read) => read,
                Err(error) => {
                    self.completion.begin_flush();
                    self.poisoned = unsafe {
                        self.reader
                            .Flush(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                    }
                    .is_err()
                        || !self.completion.wait_flush();
                    self.current = None;
                    return Err(error);
                }
            };
            let (flags, pts, sample) = (read.flags, read.pts, read.sample);
            ensure!(!cancelled(), "native preview seek superseded");
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return self
                    .current
                    .clone()
                    .map(Some)
                    .context("native preview reached end before requested frame");
            }
            if let Some(sample) = sample {
                let pts_ms = pts as f64 / 10_000.0;
                // FFmpeg's random seek returns the first frame at/after the
                // requested time. Continuous playback keeps nearest-frame
                // selection, as the existing preview path does.
                if pts_ms + if jump { 0.001 } else { half } < at {
                    continue;
                }
                let image = self.read_image(&sample, pts_ms)?;
                self.current = Some(Arc::new(image));
                return Ok(self.current.clone());
            }
        }
        bail!("native preview exceeded sample limit")
    }
    fn read_image(&mut self, sample: &IMFSample, pts_ms: f64) -> Result<Frame> {
        let buffer = unsafe { sample.GetBufferByIndex(0)? };
        let dxgi: IMFDXGIBuffer = buffer
            .cast()
            .context("native decoder did not produce a GPU preview; use FFmpeg")?;
        let mut raw = std::ptr::null_mut();
        unsafe {
            dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
        }
        ensure!(!raw.is_null(), "native preview has no texture");
        let texture = unsafe { ID3D11Texture2D::from_raw(raw) };
        let subresource = unsafe { dxgi.GetSubresourceIndex()? };
        let bgra = self.scaler.read(&texture, subresource)?;
        let (width, height) = self.size;
        Ok(Frame {
            pts_ms,
            width,
            height,
            bgra,
        })
    }
}
