//! Native GPU MP4 encoding with bounded surface pools, quality controls and
//! cancellable asynchronous finalization. Export owns audio mux and publication.
use super::{
    Runtime,
    convert::Converter,
    finalize::{self, Completion},
    hardware_encoders, string,
};
use crate::{
    media::writer::WriterSpec,
    project::model::Codec,
    render::{Gpu, dx12::NativeFrame},
};
use anyhow::{Context, Result, ensure};
use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows62::{
    Win32::{Media::MediaFoundation::*, System::Com::IPersist},
    core::{GUID, Interface, PCWSTR},
};

struct Pending(PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
struct Transforms(Vec<IMFTransform>);
impl Drop for Transforms {
    fn drop(&mut self) {
        for transform in &self.0 {
            if let Ok(shutdown) = transform.cast::<IMFShutdown>() {
                unsafe {
                    let _ = shutdown.Shutdown();
                }
            }
        }
    }
}

pub struct NativeWriter {
    writer: Option<IMFSinkWriter>,
    _transforms: Transforms,
    samples: super::samples::Samples,
    // Keep the manager and device alive until the sink/encoder has gone away.
    _manager: IMFDXGIDeviceManager,
    converter: Converter,
    interop: super::device::Device,
    output: PathBuf,
    stream: u32,
    fps: u32,
    frames: u64,
    encoder_name: String,
    completion: std::sync::Arc<Completion>,
    _runtime: Runtime,
    // Remove staging only after all native objects/runtime have been released.
    pending: Pending,
}

impl Drop for NativeWriter {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            // Abort queued samples before releasing the sink and its file.
            // Successful finalization already took the writer out.
            unsafe {
                let _ = writer.Flush(self.stream);
            }
            drop(writer);
        }
    }
}

fn video_type(width: u32, height: u32, fps: u32, subtype: &GUID) -> Result<IMFMediaType> {
    unsafe {
        let media = MFCreateMediaType()?;
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        media.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
        media.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
        media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        media.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        media.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        media.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        Ok(media)
    }
}

impl NativeWriter {
    pub fn create(output: &Path, spec: &WriterSpec, gpu: &Gpu) -> Result<Self> {
        Self::create_with(output, spec, || super::device::Device::render_device(gpu))
    }
    pub fn create_for_capture(
        output: &Path,
        spec: &WriterSpec,
        device: windows62::Win32::Graphics::Direct3D11::ID3D11Device,
    ) -> Result<Self> {
        Self::create_with(output, spec, || super::device::Device::capture(device))
    }
    fn create_with(
        output: &Path,
        spec: &WriterSpec,
        device: impl FnOnce() -> Result<super::device::Device>,
    ) -> Result<Self> {
        ensure!(
            !spec.audio,
            "native video encoder requires separate audio muxing"
        );
        ensure!(
            spec.quality
                .is_none_or(|quality| quality.is_finite() && (0.0..=1.0).contains(&quality)),
            "invalid native encoder quality"
        );
        ensure!(
            spec.width > 0
                && spec.height > 0
                && spec.width % 2 == 0
                && spec.height % 2 == 0
                && spec.fps > 0,
            "invalid native encoder size or frame rate"
        );
        let subtype = match spec.codec {
            Codec::H264 => MFVideoFormat_H264,
            Codec::Hevc => MFVideoFormat_HEVC,
            Codec::Prores => anyhow::bail!("native ProRes encoding is unavailable"),
        };
        ensure!(
            spec.bitrate > 0 && spec.bitrate <= u32::MAX as u64,
            "invalid native encoder bitrate"
        );
        let runtime = Runtime::new()?;
        let interop = device()?;
        let hardware = hardware_encoders(interop.adapter_luid(), spec.codec)?;
        ensure!(
            !hardware.is_empty(),
            "no native hardware {:?} encoder for this GPU",
            spec.codec
        );
        let converter = Converter::new_device(
            interop.device(),
            interop.context(),
            spec.width,
            spec.height,
            spec.fps,
        )?;
        let (mut token, mut manager) = (0, None);
        unsafe {
            MFCreateDXGIDeviceManager(&mut token, &mut manager)
                .context("creating DXGI media manager")?;
        }
        let manager = manager.context("creating native encoder device manager")?;
        unsafe {
            manager
                .ResetDevice(interop.device(), token)
                .context("assigning encoder D3D11 device")?;
        }
        let mut attributes = None;
        unsafe {
            MFCreateAttributes(&mut attributes, 3)?;
        }
        let attributes = attributes.context("creating native encoder attributes")?;
        let (callback, completion) = finalize::callback();
        unsafe {
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            // Color conversion is already done by our D3D11 GPU processor.
            // Windows rejects DISABLE_CONVERTERS combined with hardware
            // transforms. Inspect every selected transform below instead.
            attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)?;
            attributes.SetUnknown(&MF_SINK_WRITER_ASYNC_CALLBACK, &callback)?;
        }
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stage = output.with_extension(format!(
            "{}.{}.{}.native.mp4",
            std::process::id(),
            crate::util::host_now().to_bits(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        // Claim cleanup only after exclusive creation succeeds.
        drop(
            std::fs::File::options()
                .write(true)
                .create_new(true)
                .open(&stage)?,
        );
        let pending = Pending(stage);
        let mut wide: Vec<u16> = pending.0.as_os_str().encode_wide().collect();
        ensure!(
            !wide.contains(&0),
            "native encoder output path contains NUL"
        );
        wide.push(0);
        let mut transforms = Transforms(Vec::new());
        let writer = unsafe {
            MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, &attributes)
                .context("creating native MP4 sink")?
        };
        let output_type = video_type(spec.width, spec.height, spec.fps, &subtype)?;
        unsafe {
            output_type.SetUINT32(&MF_MT_AVG_BITRATE, spec.bitrate as u32)?;
        }
        let stream = unsafe {
            writer
                .AddStream(&output_type)
                .context("adding native compressed video stream")?
        };
        let input_type = video_type(spec.width, spec.height, spec.fps, &MFVideoFormat_NV12)?;
        let samples = super::samples::Samples::new(&manager, &input_type)?;
        let parameters = if let Some(quality) = spec.quality {
            let mut parameters = None;
            unsafe {
                MFCreateAttributes(&mut parameters, 2)?;
            }
            let parameters = parameters.context("creating native quality parameters")?;
            unsafe {
                parameters.SetUINT32(
                    &CODECAPI_AVEncCommonRateControlMode,
                    eAVEncCommonRateControlMode_Quality.0 as u32,
                )?;
                parameters.SetUINT32(
                    &CODECAPI_AVEncCommonQuality,
                    (quality * 100.0).round() as u32,
                )?;
            }
            Some(parameters)
        } else {
            None
        };
        unsafe {
            writer
                .SetInputMediaType(stream, &input_type, parameters.as_ref())
                .context("setting native NV12 encoder input")?;
        }
        let extended: IMFSinkWriterEx = writer.cast()?;
        let mut encoder_name = None;
        let mut exhausted = false;
        for index in 0..8 {
            let mut transform = None;
            let mut category = GUID::zeroed();
            let result = unsafe {
                extended.GetTransformForStream(stream, index, Some(&mut category), &mut transform)
            };
            if result
                .as_ref()
                .is_err_and(|error| error.code() == MF_E_INVALIDINDEX)
                && index > 0
            {
                exhausted = true;
                break;
            }
            result.context("inspecting native encoder pipeline")?;
            let transform = transform.context("native encoder returned no transform")?;
            transforms.0.push(transform.clone());
            let attributes = unsafe {
                transform
                    .GetAttributes()
                    .context("querying selected native transform")?
            };
            let aware = unsafe { attributes.GetUINT32(&MF_SA_D3D11_AWARE) }.unwrap_or(0) != 0;
            ensure!(aware, "native transform {category:?} is not D3D11-aware");
            if category == MFT_CATEGORY_VIDEO_ENCODER {
                let class_id = unsafe { attributes.GetGUID(&MFT_TRANSFORM_CLSID_Attribute) }
                    .ok()
                    .or_else(|| {
                        transform
                            .cast::<IPersist>()
                            .ok()
                            .and_then(|p| unsafe { p.GetClassID() }.ok())
                    });
                let url = string(&attributes, &MFT_ENUM_HARDWARE_URL_Attribute).ok();
                let selected = hardware.iter().find(|e| {
                    Some(e.class_id) == class_id || url.as_ref() == Some(&e.hardware_url)
                });
                ensure!(
                    selected.is_some(),
                    "native encoder hardware identity is unverified: class={class_id:?}, url={url:?}"
                );
                encoder_name = selected.map(|e| e.name.clone());
            }
        }
        ensure!(
            exhausted,
            "native encoder pipeline exceeds inspection limit"
        );
        let encoder_name =
            encoder_name.context("no verified hardware encoder in native pipeline")?;
        unsafe {
            writer
                .BeginWriting()
                .context("starting native GPU encoding")?;
        }
        Ok(Self {
            writer: Some(writer),
            _transforms: transforms,
            samples,
            _manager: manager,
            converter,
            interop,
            output: output.into(),
            pending,
            stream,
            fps: spec.fps,
            frames: 0,
            encoder_name,
            completion,
            _runtime: runtime,
        })
    }
    pub fn encoder_name(&self) -> &str {
        &self.encoder_name
    }

    pub fn gpu_memory_usage(&self) -> Result<(u64, u64)> {
        let info = self.interop.local_memory()?;
        Ok((info.CurrentUsage, info.Budget))
    }

    pub fn render(
        &self,
        width: u32,
        height: u32,
        render: impl FnOnce(&wgpu::TextureView, &mut wgpu::CommandEncoder),
    ) -> Result<NativeFrame> {
        self.interop.render(width, height, render)
    }
    pub fn push(&mut self, frame: &NativeFrame) -> Result<()> {
        self.push_cancellable(frame, &|| false)
    }
    pub fn push_cancellable(
        &mut self,
        frame: &NativeFrame,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        ensure!(
            frame.size() == self.converter.output_size(),
            "GPU encoder frame dimensions changed"
        );
        self.push_texture(frame.texture(), cancelled)
    }
    pub fn push_texture(
        &mut self,
        texture: &windows62::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        let mut desc = windows62::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut desc);
        }
        if self.converter.input_size() != (desc.Width, desc.Height) {
            self.converter = Converter::new_sized(
                self.interop.device(),
                self.interop.context(),
                (desc.Width, desc.Height),
                self.converter.output_size(),
                self.fps,
            )?;
        }
        ensure!(
            self.frames <= u32::MAX as u64,
            "native encoder frame index overflow"
        );
        let (sample, output, subresource) = self.samples.acquire(cancelled)?;
        self.converter
            .convert_texture(texture, &output, subresource, self.frames as u32)?;
        self.interop.wait_for_native()?;
        unsafe {
            let start = self.frames as i64 * 10_000_000 / self.fps as i64;
            let end = (self.frames + 1) as i64 * 10_000_000 / self.fps as i64;
            sample.SetSampleTime(start)?;
            sample.SetSampleDuration(end - start)?;
            self.writer
                .as_ref()
                .context("native encoder already closed")?
                .WriteSample(self.stream, &sample)?;
        }
        self.frames += 1;
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        self.finish_cancellable(&|| false)
    }
    pub fn finish_cancellable(mut self, cancelled: &dyn Fn() -> bool) -> Result<()> {
        ensure!(self.frames > 0, "native encoder has no frames");
        ensure!(!cancelled(), "native encoding cancelled");
        let writer = self
            .writer
            .as_ref()
            .context("native encoder already closed")?;
        unsafe {
            writer.Finalize().context("finalizing native GPU video")?;
        }
        self.completion
            .wait(cancelled, std::time::Duration::from_secs(60))?;
        drop(self.writer.take());
        ensure!(!cancelled(), "native encoding cancelled");
        std::fs::rename(&self.pending.0, &self.output).context("publishing native GPU video")?;
        Ok(())
    }
}
