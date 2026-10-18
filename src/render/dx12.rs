//! Bounded GPU-only render handoff to a matching D3D11 media device.
//! Shared surfaces are reused only after native reads have completed.
use super::Gpu;
use anyhow::{Context, Result, ensure};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};
use windows62::{
    Win32::{
        Foundation::{CloseHandle, GENERIC_ALL, HANDLE, HMODULE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
        System::Threading::{CreateEventW, WaitForSingleObject},
    },
    core::{Interface, PCWSTR},
};

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct GpuInterop {
    gpu: Gpu,
    raw: ID3D12Device,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    native_context: ID3D11DeviceContext4,
    fence: ID3D11Fence,
    event: OwnedHandle,
    sequence: Cell<u64>,
    surfaces: RefCell<Vec<Arc<Surface>>>,
    adapter: IDXGIAdapter3,
}

/// Rendered and synchronized for native media use. No CPU pixel buffer exists.
pub struct NativeFrame {
    surface: Arc<Surface>,
}
struct Surface {
    // Retain wgpu's imported allocation until the frame is released.
    allocation: wgpu::Texture,
    view: wgpu::TextureView,
    native: ID3D11Texture2D,
    size: (u32, u32),
}
impl Drop for GpuInterop {
    fn drop(&mut self) {
        // D3D11 contexts can retain bound views/resources. Unbind and submit
        // their release before dropping the imported shared allocations.
        unsafe {
            self.context.ClearState();
            self.context.Flush();
        }
        self.surfaces.get_mut().clear();
        let _ = self.gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(2)),
        });
    }
}
impl NativeFrame {
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.surface.native
    }
    pub fn size(&self) -> (u32, u32) {
        self.surface.size
    }
}

impl GpuInterop {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let raw = unsafe { gpu.device.as_hal::<wgpu::hal::api::Dx12>() }
            .context("native GPU media requires the DX12 backend")?
            .raw_device()
            .clone();
        let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        let adapter: IDXGIAdapter = unsafe { factory.EnumAdapterByLuid(raw.GetAdapterLuid())? };
        let (mut device, mut context) = (None, None);
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device = device.context("D3D11 media device")?;
        let context = context.context("D3D11 media context")?;
        let mut fence: Option<ID3D11Fence> = None;
        unsafe {
            device
                .cast::<ID3D11Device5>()?
                .CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence)?;
        }
        let event = OwnedHandle(unsafe { CreateEventW(None, false, false, None)? });
        Ok(Self {
            gpu: gpu.clone(),
            raw,
            device,
            native_context: context.cast()?,
            context,
            fence: fence.context("creating native GPU completion fence")?,
            event,
            sequence: Cell::new(0),
            surfaces: RefCell::new(Vec::new()),
            adapter: adapter.cast()?,
        })
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    pub fn adapter_luid(&self) -> windows62::Win32::Foundation::LUID {
        unsafe { self.raw.GetAdapterLuid() }
    }

    pub fn local_memory(&self) -> Result<DXGI_QUERY_VIDEO_MEMORY_INFO> {
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        unsafe {
            self.adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)?;
        }
        Ok(info)
    }

    /// Complete native reads before a released frame can return to the pool.
    pub fn wait_for_native(&self) -> Result<()> {
        let value = self
            .sequence
            .get()
            .checked_add(1)
            .context("native GPU fence exhausted")?;
        self.sequence.set(value);
        unsafe {
            self.native_context.Signal(&self.fence, value)?;
            self.context.Flush();
            self.fence.SetEventOnCompletion(value, self.event.0)?;
            let status = WaitForSingleObject(self.event.0, 10_000);
            if status == WAIT_TIMEOUT {
                anyhow::bail!("native GPU conversion timed out");
            }
            ensure!(
                status == WAIT_OBJECT_0,
                "waiting for native GPU conversion failed"
            );
        }
        Ok(())
    }

    /// Render a pooled frame and hand it off after the GPU reaches COMMON.
    /// Finish native GPU reads before releasing the frame for reuse.
    pub fn render(
        &self,
        width: u32,
        height: u32,
        render: impl FnOnce(&wgpu::TextureView, &mut wgpu::CommandEncoder),
    ) -> Result<NativeFrame> {
        let limits = self.gpu.device.limits();
        ensure!(
            width > 0
                && height > 0
                && width <= limits.max_texture_dimension_2d
                && height <= limits.max_texture_dimension_2d,
            "invalid native GPU frame dimensions"
        );
        let cached = self
            .surfaces
            .borrow()
            .iter()
            .find(|surface| surface.size == (width, height) && Arc::strong_count(surface) == 1)
            .cloned();
        let surface = match cached {
            Some(surface) => surface,
            None => {
                ensure!(
                    self.surfaces.borrow().len() < 3,
                    "native surface pool is full; release outstanding frames"
                );
                let surface = Arc::new(self.allocate_surface(width, height)?);
                self.surfaces.borrow_mut().push(surface.clone());
                surface
            }
        };
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        // Native consumers may read every pixel, even when the compositor
        // chooses to draw only part of the target.
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        render(&surface.view, &mut encoder);
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &surface.allocation,
                selector: None,
                state: wgpu::TextureUses::PRESENT,
            }),
        );
        let submission = self.gpu.queue.submit([encoder.finish()]);
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .context("waiting for GPU media handoff")?;
        Ok(NativeFrame { surface })
    }

    fn allocate_surface(&self, width: u32, height: u32) -> Result<Surface> {
        let mut resource: Option<ID3D12Resource> = None;
        unsafe {
            self.raw.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    CreationNodeMask: 1,
                    VisibleNodeMask: 1,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_SHARED,
                &D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: width as u64,
                    Height: height,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                    Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                    ..Default::default()
                },
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )?;
        }
        let resource = resource.context("creating shared render target")?;
        let handle = OwnedHandle(unsafe {
            self.raw
                .CreateSharedHandle(&resource, None, GENERIC_ALL.0, PCWSTR::null())?
        });
        let native: ID3D11Texture2D = unsafe {
            self.device
                .cast::<ID3D11Device1>()?
                .OpenSharedResource1(handle.0)?
        };
        drop(handle);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        // SAFETY: resource is BGRA, single-layer/mip/sample, on the same raw
        // device. It starts in COMMON; wgpu owns and tracks its GPU use below.
        let hal = unsafe {
            wgpu::hal::dx12::Device::texture_from_raw(
                resource,
                wgpu::TextureFormat::Bgra8Unorm,
                wgpu::TextureDimension::D2,
                size,
                1,
                1,
            )
        };
        let texture = unsafe {
            self.gpu
                .device
                .create_texture_from_hal::<wgpu::hal::api::Dx12>(
                    hal,
                    &wgpu::TextureDescriptor {
                        label: Some("native media surface"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Bgra8Unorm,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::UNINITIALIZED,
                )
        };
        let view = texture.create_view(&Default::default());
        Ok(Surface {
            allocation: texture,
            view,
            native,
            size: (width, height),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gpu() -> Gpu {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        eprintln!("GPU surface adapter: {:?}", adapter.get_info());
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        Gpu {
            instance,
            adapter,
            device,
            queue,
        }
    }
    fn handles() -> u32 {
        let mut count = 0;
        unsafe {
            windows62::Win32::System::Threading::GetProcessHandleCount(
                windows62::Win32::System::Threading::GetCurrentProcess(),
                &mut count,
            )
            .unwrap();
        }
        count
    }
    #[test]
    #[ignore = "shares rendered GPU surfaces between native DX12 and D3D11 on the connected PC"]
    fn rendered_surfaces_reach_d3d11_without_pixel_upload_or_handle_leaks() {
        let gpu = gpu();
        let interop = GpuInterop::new(&gpu).unwrap();
        let native_adapter: IDXGIAdapter = unsafe {
            interop
                .device
                .cast::<IDXGIDevice>()
                .unwrap()
                .GetAdapter()
                .unwrap()
        };
        let native_luid = unsafe { native_adapter.GetDesc().unwrap().AdapterLuid };
        let raw_luid = unsafe { interop.raw.GetAdapterLuid() };
        assert_eq!(
            (native_luid.LowPart, native_luid.HighPart),
            (raw_luid.LowPart, raw_luid.HighPart)
        );
        let mut before = 0;
        for i in 0..40 {
            let red = i % 2 == 0;
            let frame = interop
                .render(66, 38, |view, encoder| {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(if red {
                                    wgpu::Color::RED
                                } else {
                                    wgpu::Color::BLUE
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                })
                .unwrap();
            assert_eq!(frame.size(), (66, 38));
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                frame.texture().GetDesc(&mut desc);
            }
            assert_eq!(desc.Format, DXGI_FORMAT_B8G8R8A8_UNORM);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.MiscFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            unsafe {
                interop
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut staging))
                    .unwrap();
            }
            let staging = staging.unwrap();
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                interop.context.CopyResource(&staging, frame.texture());
                interop
                    .context
                    .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .unwrap();
                for y in 0..38usize {
                    let row = std::slice::from_raw_parts(
                        (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                        66 * 4,
                    );
                    for pixel in row.chunks_exact(4) {
                        assert_eq!(
                            pixel,
                            if red {
                                &[0, 0, 255, 255]
                            } else {
                                &[255, 0, 0, 255]
                            }
                        );
                    }
                }
                interop.context.Unmap(&staging, 0);
            }
            if i == 7 {
                // D3D/wgpu allocate persistent events and pipeline objects
                // on first use. Compare after that warmup, with the same
                // live-frame resources on both sides.
                before = handles();
            }
            if i >= 8 {
                assert!(
                    handles() <= before + 2,
                    "shared handles accumulated: {before} -> {} at frame {i}",
                    handles()
                );
            }
        }
        unsafe {
            interop.context.ClearState();
            interop.context.Flush();
        }
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        eprintln!(
            "GPU surface handles: {before} after warmup, {} after 40 frames",
            handles()
        );
        assert!(
            handles() <= before + 4,
            "shared handles leaked: {before} -> {}",
            handles()
        );
        assert!(interop.render(0, 32, |_, _| {}).is_err());
        let first = interop.render(66, 38, |_, _| {}).unwrap();
        let identity = first.texture().as_raw();
        drop(first);
        let reused = interop.render(66, 38, |_, _| {}).unwrap();
        assert_eq!(
            reused.texture().as_raw(),
            identity,
            "released surfaces must be reused"
        );
        let second = interop.render(66, 38, |_, _| {}).unwrap();
        let third = interop.render(66, 38, |_, _| {}).unwrap();
        assert!(
            interop.render(66, 38, |_, _| {}).is_err(),
            "outstanding surfaces must stay bounded"
        );
        drop((reused, second, third));
        assert!(interop.render(66, 38, |_, _| {}).is_ok());
        assert!(
            interop
                .render(
                    gpu.device.limits().max_texture_dimension_2d + 1,
                    32,
                    |_, _| {}
                )
                .is_err()
        );
    }
}
