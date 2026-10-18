//! Bounded encoder-input surfaces recycled only after the MFT releases samples.
use anyhow::{Context, Result, ensure};
use windows62::{
    Win32::{Graphics::Direct3D11::*, Media::MediaFoundation::*},
    core::Interface,
};

pub(super) struct Samples(IMFVideoSampleAllocatorEx);
impl Samples {
    pub fn new(manager: &IMFDXGIDeviceManager, media: &IMFMediaType) -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        unsafe {
            MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut raw)?;
        }
        ensure!(!raw.is_null(), "native sample allocator was not created");
        let allocator = unsafe { IMFVideoSampleAllocatorEx::from_raw(raw) };
        let mut attributes = None;
        unsafe {
            MFCreateAttributes(&mut attributes, 2)?;
        }
        let attributes = attributes.context("creating native sample pool attributes")?;
        unsafe {
            attributes.SetUINT32(&MF_SA_D3D11_BINDFLAGS, D3D11_BIND_RENDER_TARGET.0 as u32)?;
            allocator.SetDirectXManager(manager)?;
            allocator.InitializeSampleAllocatorEx(4, 16, &attributes, media)?;
        }
        let pool = Self(allocator);
        // Validate actual GPU allocation before production selects this route.
        let _ = pool.acquire(&|| false)?;
        Ok(pool)
    }
    pub fn acquire(
        &self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(IMFSample, ID3D11Texture2D, u32)> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let sample = loop {
            ensure!(!cancelled(), "native encoding cancelled");
            match unsafe { self.0.AllocateSample() } {
                Ok(sample) => break sample,
                Err(error) if error.code() == MF_E_SAMPLEALLOCATOR_EMPTY => {
                    ensure!(
                        std::time::Instant::now() < deadline,
                        "native encoder sample pool timed out"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(error) => return Err(error).context("allocating native encoder sample"),
            }
        };
        let buffer = unsafe { sample.GetBufferByIndex(0)? };
        let dxgi: IMFDXGIBuffer = buffer
            .cast()
            .context("sample pool did not produce GPU memory")?;
        let mut raw = std::ptr::null_mut();
        unsafe {
            dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
        }
        ensure!(!raw.is_null(), "native encoder sample has no texture");
        let texture = unsafe { ID3D11Texture2D::from_raw(raw) };
        let subresource = unsafe { dxgi.GetSubresourceIndex()? };
        let length = unsafe { buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()? };
        unsafe {
            buffer.SetCurrentLength(length)?;
        }
        Ok((sample, texture, subresource))
    }
}
impl Drop for Samples {
    fn drop(&mut self) {
        unsafe {
            if let Err(error) = self.0.UninitializeSampleAllocator() {
                eprintln!("native sample uninitialize: {error}");
            }
            if let Err(error) = self.0.SetDirectXManager(None) {
                eprintln!("native sample device release: {error}");
            }
        }
    }
}
