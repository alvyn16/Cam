//! Adapter-specific hardware discovery and native GPU video encoding.
//! Export policy retains FFmpeg when the native capability checks fail.
use anyhow::{Context, Result};
mod convert;
mod finalize;
mod samples;
mod writer;
use windows62::{
    Win32::{
        Foundation::{LUID, RPC_E_CHANGED_MODE},
        Media::MediaFoundation::*,
        System::Com::*,
    },
    core::GUID,
};
pub use writer::NativeWriter;

struct Runtime {
    uninitialize: bool,
    // COM and MF must be shut down on the creating thread.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Runtime {
    fn new() -> Result<Self> {
        let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if status.is_err() && status != RPC_E_CHANGED_MODE {
            status.ok().context("initializing encoder COM")?;
        }
        let uninitialize = status.is_ok();
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
            if uninitialize {
                unsafe {
                    CoUninitialize();
                }
            }
            return Err(error).context("starting native media runtime");
        }
        Ok(Self {
            uninitialize,
            _thread: std::marker::PhantomData,
        })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            if self.uninitialize {
                CoUninitialize();
            }
        }
    }
}
struct Activations {
    pointer: *mut Option<IMFActivate>,
    len: u32,
}
impl Drop for Activations {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            unsafe {
                for activation in std::slice::from_raw_parts_mut(self.pointer, self.len as usize) {
                    activation.take();
                }
                CoTaskMemFree(Some(self.pointer.cast()));
            }
        }
    }
}

#[derive(Debug)]
pub struct HardwareEncoder {
    pub name: String,
    pub class_id: GUID,
    pub hardware_url: String,
}

fn string(attributes: &IMFAttributes, key: &GUID) -> Result<String> {
    let len = unsafe { attributes.GetStringLength(key)? };
    anyhow::ensure!(len <= 65536, "native encoder attribute is too long");
    let mut buffer = vec![0u16; len as usize + 1];
    unsafe {
        attributes.GetString(key, &mut buffer, None)?;
    }
    Ok(String::from_utf16_lossy(&buffer[..len as usize]))
}

/// Enumerate only hardware encoders attached to the specified GPU. LUID is a
/// blob (not a UINT64), as required by MFTEnum2's adapter filter.
pub fn hardware_encoders(
    adapter: LUID,
    codec: crate::project::model::Codec,
) -> Result<Vec<HardwareEncoder>> {
    use crate::project::model::Codec;
    let subtype = match codec {
        Codec::H264 => MFVideoFormat_H264,
        Codec::Hevc => MFVideoFormat_HEVC,
        Codec::Prores => return Ok(Vec::new()),
    };
    let _runtime = Runtime::new()?;
    let mut attributes = None;
    unsafe {
        MFCreateAttributes(&mut attributes, 1)?;
    }
    let attributes = attributes.context("creating encoder discovery attributes")?;
    let mut luid = [0u8; 8];
    luid[..4].copy_from_slice(&adapter.LowPart.to_le_bytes());
    luid[4..].copy_from_slice(&adapter.HighPart.to_le_bytes());
    unsafe {
        attributes.SetBlob(&MFT_ENUM_ADAPTER_LUID, &luid)?;
    }
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype,
    };
    let (mut pointer, mut len) = (std::ptr::null_mut(), 0);
    let result = unsafe {
        MFTEnum2(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&output),
            &attributes,
            &mut pointer,
            &mut len,
        )
    };
    let activations = Activations { pointer, len };
    result.context("enumerating native hardware encoders")?;
    if activations.pointer.is_null() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for activation in
        unsafe { std::slice::from_raw_parts(activations.pointer, activations.len as usize) }
            .iter()
            .flatten()
    {
        // Enumeration must not instantiate a disposable hardware encoder.
        // Awareness is verified on the actual sink transform before use.
        found.push(HardwareEncoder {
            name: string(activation, &MFT_FRIENDLY_NAME_Attribute)?,
            class_id: unsafe { activation.GetGUID(&MFT_TRANSFORM_CLSID_Attribute)? },
            hardware_url: string(activation, &MFT_ENUM_HARDWARE_URL_Attribute)?,
        });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gpu() -> crate::render::Gpu {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        crate::render::Gpu {
            instance,
            adapter,
            device,
            queue,
        }
    }

    #[test]
    #[ignore = "checks that native constant-quality settings change independently decoded detail"]
    fn native_quality_changes_size_and_decoded_error() {
        use crate::{
            engine::scene::Scene,
            project::model::Project,
            render::{Renderer, VideoFrame},
        };
        let gpu = gpu();
        let dir = std::env::temp_dir().join(format!(
            "cam-native-quality-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&dir).unwrap();
        let mut project = Project::default();
        project.style.padding = 0.0;
        project.style.corner_radius = 0.0;
        project.style.shadow.intensity = 0.0;
        project.style.cursor.hide = true;
        project.style.motion_blur.amount = 0.0;
        let recording = crate::synth::recording(640, 400, 1000.0);
        let mut scene = Scene::new(project.clone(), &recording, 10.0);
        let frame = scene.frame(0.0, 400.0);
        let mut renderer = Renderer::new(gpu.clone());
        renderer.prepare(&project.style, &recording, None);
        let mut seed = 42u32;
        let mut pixels = vec![0u8; 640 * 400 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            pixel.copy_from_slice(&[seed as u8, seed as u8, seed as u8, 255]);
        }
        renderer.set_screen(&VideoFrame {
            width: 640,
            height: 400,
            stride: 640 * 4,
            bgra: &pixels,
        });
        // This readback is the test oracle; production encoding never uses it.
        let reference = renderer.render_to_image(&frame, &project.style);
        for codec in [
            crate::project::model::Codec::H264,
            crate::project::model::Codec::Hevc,
        ] {
            let mut results = Vec::new();
            for quality in [0.95, 0.55] {
                let output = dir.join(format!("{codec:?}-{quality}.mp4"));
                let mut writer = NativeWriter::create(
                    &output,
                    &crate::media::writer::WriterSpec {
                        width: reference.width,
                        height: reference.height,
                        fps: 10,
                        codec,
                        bitrate: 8_000_000,
                        quality: Some(quality),
                        audio: false,
                    },
                    &gpu,
                )
                .unwrap();
                for _ in 0..10 {
                    let target = writer
                        .render(reference.width, reference.height, |target, encoder| {
                            renderer.render(&frame, &project.style, target, encoder);
                        })
                        .unwrap();
                    writer.push(&target).unwrap();
                }
                writer.finish().unwrap();
                let decoded = crate::media::ffmpeg::output(
                    crate::media::ffmpeg::command("ffmpeg")
                        .args(["-v", "error", "-i"])
                        .arg(&output)
                        .args([
                            "-frames:v",
                            "1",
                            "-pix_fmt",
                            "rgba",
                            "-f",
                            "rawvideo",
                            "pipe:1",
                        ]),
                )
                .unwrap()
                .stdout;
                assert_eq!(decoded.len(), reference.rgba.len());
                let mse = decoded
                    .chunks_exact(4)
                    .zip(reference.rgba.chunks_exact(4))
                    .map(|(a, b)| (a[0] as f64 - b[0] as f64).powi(2))
                    .sum::<f64>()
                    / (reference.width * reference.height) as f64;
                let bytes = output.metadata().unwrap().len();
                eprintln!(
                    "Native {codec:?} quality={quality}: bytes={bytes}, decoded MSE={mse:.3}"
                );
                results.push((bytes, mse));
            }
            assert!(
                results[0].0 > results[1].0,
                "quality did not affect file size: {results:?}"
            );
            assert!(
                results[1].1 > results[0].1 + 1.0,
                "quality did not preserve more detail: {results:?}"
            );
        }
        eprintln!("Native quality artifacts: {}", dir.display());
    }

    #[test]
    #[ignore = "checks native MP4 sink attribute compatibility on the connected PC"]
    fn native_sink_configuration() {
        use std::os::windows::ffi::OsStrExt;
        use windows62::core::PCWSTR;
        let _runtime = Runtime::new().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "cam-mf-sink-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&dir).unwrap();
        let gpu = gpu();
        let interop = crate::render::dx12::GpuInterop::new(&gpu).unwrap();
        let (mut token, mut manager) = (0, None);
        unsafe {
            MFCreateDXGIDeviceManager(&mut token, &mut manager).unwrap();
        }
        let manager = manager.unwrap();
        unsafe {
            manager.ResetDevice(interop.device(), token).unwrap();
        }
        for configuration in 0..5 {
            let file = dir.join(format!("case-{configuration}.mp4"));
            let mut wide: Vec<_> = file.as_os_str().encode_wide().collect();
            wide.push(0);
            let mut attributes = None;
            unsafe {
                MFCreateAttributes(&mut attributes, 4).unwrap();
            }
            let attributes = attributes.unwrap();
            unsafe {
                if configuration >= 1 {
                    attributes
                        .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                        .unwrap();
                }
                if configuration == 2 {
                    attributes
                        .SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)
                        .unwrap();
                }
                if configuration >= 3 {
                    attributes
                        .SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)
                        .unwrap();
                }
                if configuration >= 4 {
                    attributes
                        .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
                        .unwrap();
                }
                let result = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, &attributes);
                eprintln!(
                    "MP4 sink configuration {configuration}: {:?}",
                    result.as_ref().map(|_| "ok")
                );
                if configuration == 2 {
                    assert!(
                        result.is_err(),
                        "Windows must reject hardware plus DISABLE_CONVERTERS"
                    );
                } else {
                    assert!(
                        result.is_ok(),
                        "valid native sink configuration {configuration}: {result:?}"
                    );
                }
                drop(result);
            }
        }
        eprintln!("Sink diagnostics: {}", dir.display());
    }

    #[test]
    #[ignore = "encodes real GPU surfaces through native H.264/HEVC hardware and independently decodes them"]
    fn native_gpu_encode_preserves_frames_and_destination() {
        let gpu = gpu();
        let dir = std::env::temp_dir().join(format!(
            "cam-mf-encode-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&dir).unwrap();
        for (codec, name) in [
            (crate::project::model::Codec::H264, "h264"),
            (crate::project::model::Codec::Hevc, "hevc"),
        ] {
            let output = dir.join(format!("{name}.mp4"));
            std::fs::write(&output, b"existing destination").unwrap();
            let spec = crate::media::writer::WriterSpec {
                width: 320,
                height: 180,
                fps: 30,
                codec,
                bitrate: 2_000_000,
                quality: None,
                audio: false,
            };
            for frames in [0, 3] {
                let mut cancelled = NativeWriter::create(&output, &spec, &gpu).unwrap();
                for _ in 0..frames {
                    let black = cancelled.render(320, 180, |_, _| {}).unwrap();
                    cancelled.push(&black).unwrap();
                }
                drop(cancelled);
                assert_eq!(std::fs::read(&output).unwrap(), b"existing destination");
                assert_eq!(
                    std::fs::read_dir(&dir).unwrap().count(),
                    if name == "h264" { 1 } else { 2 }
                );
            }
            for after_finalize in [false, true] {
                let mut cancelled = NativeWriter::create(&output, &spec, &gpu).unwrap();
                for _ in 0..3 {
                    let frame = cancelled.render(320, 180, |_, _| {}).unwrap();
                    cancelled.push(&frame).unwrap();
                }
                let calls = std::cell::Cell::new(0);
                let started = std::time::Instant::now();
                let result = cancelled.finish_cancellable(&|| {
                    calls.set(calls.get() + 1);
                    !after_finalize || calls.get() > 1
                });
                assert!(result.unwrap_err().to_string().contains("cancelled"));
                if after_finalize {
                    assert!(calls.get() >= 2, "did not reach asynchronous finalization");
                }
                assert!(started.elapsed() < std::time::Duration::from_secs(5));
                assert_eq!(std::fs::read(&output).unwrap(), b"existing destination");
                assert_eq!(
                    std::fs::read_dir(&dir).unwrap().count(),
                    if name == "h264" { 1 } else { 2 },
                    "native cancellation left staging files"
                );
            }
            let mut writer = NativeWriter::create(&output, &spec, &gpu).unwrap();
            eprintln!(
                "Native encoder: {} -> {}",
                writer.encoder_name(),
                output.display()
            );
            for i in 0..60 {
                let color = if i < 30 {
                    wgpu::Color::RED
                } else {
                    wgpu::Color::BLUE
                };
                let frame = writer
                    .render(320, 180, |view, encoder| {
                        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(color),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            ..Default::default()
                        });
                    })
                    .unwrap();
                writer.push(&frame).unwrap();
            }
            let wrong = writer.render(322, 180, |_, _| {}).unwrap();
            assert!(writer.push(&wrong).is_err());
            assert_eq!(std::fs::read(&output).unwrap(), b"existing destination");
            writer.finish().unwrap();
            let probe = crate::media::ffmpeg::probe(&output).unwrap();
            let stream = &probe["streams"][0];
            assert_eq!(stream["codec_name"], name);
            assert_eq!(stream["width"], 320);
            assert_eq!(stream["height"], 180);
            assert_eq!(stream["nb_frames"], "60");
            assert!(
                (probe["format"]["duration"]
                    .as_str()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
                    - 2.0)
                    .abs()
                    < 0.04
            );
            for (at, expected) in [(0.5, [0i32, 0, 255]), (1.5, [255i32, 0, 0])] {
                let data = crate::media::ffmpeg::output(
                    crate::media::ffmpeg::command("ffmpeg")
                        .args(["-v", "error", "-ss", &at.to_string(), "-i"])
                        .arg(&output)
                        .args([
                            "-frames:v",
                            "1",
                            "-pix_fmt",
                            "bgra",
                            "-f",
                            "rawvideo",
                            "pipe:1",
                        ]),
                )
                .unwrap()
                .stdout;
                assert_eq!(data.len(), 320 * 180 * 4);
                for pixel in data.chunks_exact(4) {
                    for c in 0..3 {
                        assert!(
                            (pixel[c] as i32 - expected[c]).abs() < 12,
                            "{name} at {at}: {pixel:?} != {expected:?}"
                        );
                    }
                }
            }
        }
        eprintln!("Native GPU encoding artifacts: {}", dir.display());
    }

    #[test]
    #[ignore = "checks GPU-only BGRA to NV12 conversion on the connected GPU"]
    fn gpu_conversion_preserves_video_colors() {
        use windows62::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};
        let gpu = gpu();
        let interop = crate::render::dx12::GpuInterop::new(&gpu).unwrap();
        let converter = convert::Converter::new(&interop, 320, 180, 30).unwrap();
        for (color, expected) in [
            (wgpu::Color::RED, [63, 102, 240]),
            (wgpu::Color::BLUE, [32, 240, 118]),
            (wgpu::Color::BLACK, [16, 128, 128]),
            (wgpu::Color::WHITE, [235, 128, 128]),
        ] {
            let frame = interop
                .render(320, 180, |view, encoder| {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(color),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                })
                .unwrap();
            let nv12 = converter.convert(&interop, &frame, 0).unwrap();
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                nv12.GetDesc(&mut desc);
            }
            assert_eq!(desc.Format, DXGI_FORMAT_NV12);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            unsafe {
                interop
                    .device()
                    .CreateTexture2D(&desc, None, Some(&mut staging))
                    .unwrap();
            }
            let staging = staging.unwrap();
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                interop.context().CopyResource(&staging, &nv12);
                interop
                    .context()
                    .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .unwrap();
                for y in 0..270usize {
                    let row = std::slice::from_raw_parts(
                        (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                        320,
                    );
                    for (x, value) in row.iter().enumerate() {
                        let want = if y < 180 {
                            expected[0]
                        } else {
                            expected[1 + x % 2]
                        };
                        assert!(
                            (*value as i32 - want).abs() <= 3,
                            "{color:?} pixel ({x},{y}) {value} != {want}"
                        );
                    }
                }
                interop.context().Unmap(&staging, 0);
            }
        }
        assert!(convert::Converter::new(&interop, 319, 180, 30).is_err());
    }
    #[test]
    #[ignore = "enumerates and activates hardware video encoders on the connected GPU"]
    fn discover_hardware_for_the_dx12_adapter() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, _) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let raw = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
            .unwrap()
            .raw_device()
            .clone();
        let luid = unsafe { raw.GetAdapterLuid() };
        for codec in [
            crate::project::model::Codec::H264,
            crate::project::model::Codec::Hevc,
        ] {
            let found = hardware_encoders(luid, codec).unwrap();
            eprintln!(
                "Native {codec:?} encoders on {:?}: {found:#?}",
                adapter.get_info().name
            );
            assert!(
                found
                    .iter()
                    .all(|e| !e.name.is_empty() && !e.hardware_url.is_empty())
            );
        }
        assert!(
            hardware_encoders(luid, crate::project::model::Codec::Prores)
                .unwrap()
                .is_empty()
        );
    }
}
