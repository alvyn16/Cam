//! Encoder device ownership for compositor export and direct WGC capture.
use crate::render::{
    Gpu,
    dx12::{GpuInterop, NativeFrame},
};
use anyhow::{Context, Result, ensure};
use std::cell::Cell;
use windows62::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        Graphics::{Direct3D10::ID3D10Multithread, Direct3D11::*, Dxgi::*},
        System::Threading::{CreateEventW, WaitForSingleObject},
    },
    core::Interface,
};

pub(super) enum Device {
    Render(GpuInterop),
    Capture(Capture),
}
pub(super) struct Capture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    fence_context: ID3D11DeviceContext4,
    fence: ID3D11Fence,
    event: HANDLE,
    sequence: Cell<u64>,
    adapter: IDXGIAdapter3,
    luid: windows62::Win32::Foundation::LUID,
}
impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}
impl Device {
    pub fn render_device(gpu: &Gpu) -> Result<Self> {
        Ok(Self::Render(GpuInterop::new(gpu)?))
    }
    pub fn capture(device: ID3D11Device) -> Result<Self> {
        let context = unsafe { device.GetImmediateContext()? };
        unsafe {
            let _ = context
                .cast::<ID3D10Multithread>()?
                .SetMultithreadProtected(true);
        }
        let adapter: IDXGIAdapter3 =
            unsafe { device.cast::<IDXGIDevice>()?.GetAdapter()? }.cast()?;
        let luid = unsafe { adapter.GetDesc()? }.AdapterLuid;
        let mut fence = None;
        unsafe {
            device
                .cast::<ID3D11Device5>()?
                .CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence)?;
        }
        let fence_context = context.cast()?;
        let fence = fence.context("capture GPU fence")?;
        let event = unsafe { CreateEventW(None, false, false, None)? };
        Ok(Self::Capture(Capture {
            fence_context,
            device,
            context,
            fence,
            event,
            sequence: Cell::new(0),
            adapter,
            luid,
        }))
    }
    pub fn device(&self) -> &ID3D11Device {
        match self {
            Self::Render(d) => d.device(),
            Self::Capture(d) => &d.device,
        }
    }
    pub fn context(&self) -> &ID3D11DeviceContext {
        match self {
            Self::Render(d) => d.context(),
            Self::Capture(d) => &d.context,
        }
    }
    pub fn adapter_luid(&self) -> windows62::Win32::Foundation::LUID {
        match self {
            Self::Render(d) => d.adapter_luid(),
            Self::Capture(d) => d.luid,
        }
    }
    pub fn local_memory(&self) -> Result<DXGI_QUERY_VIDEO_MEMORY_INFO> {
        match self {
            Self::Render(d) => d.local_memory(),
            Self::Capture(d) => {
                let mut info = Default::default();
                unsafe {
                    d.adapter.QueryVideoMemoryInfo(
                        0,
                        DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
                        &mut info,
                    )?;
                }
                Ok(info)
            }
        }
    }
    pub fn render(
        &self,
        width: u32,
        height: u32,
        render: impl FnOnce(&wgpu::TextureView, &mut wgpu::CommandEncoder),
    ) -> Result<NativeFrame> {
        match self {
            Self::Render(d) => d.render(width, height, render),
            Self::Capture(_) => anyhow::bail!("capture encoder does not own a compositor"),
        }
    }
    pub fn wait_for_native(&self) -> Result<()> {
        match self {
            Self::Render(d) => d.wait_for_native(),
            Self::Capture(d) => unsafe {
                let next = d
                    .sequence
                    .get()
                    .checked_add(1)
                    .context("capture fence exhausted")?;
                d.sequence.set(next);
                d.fence_context.Signal(&d.fence, next)?;
                d.context.Flush();
                d.fence.SetEventOnCompletion(next, d.event)?;
                ensure!(
                    WaitForSingleObject(d.event, 10_000) == WAIT_OBJECT_0,
                    "capture GPU conversion timed out"
                );
                Ok(())
            },
        }
    }
}
