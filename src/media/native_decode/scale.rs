//! Fused GPU NV12 conversion/downsampling. Only reduced BGRA pixels reach CPU.
use anyhow::{Context, Result, ensure};
use windows62::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};

pub(super) struct Scale {
    context: ID3D11DeviceContext,
    input_texture: ID3D11Texture2D,
    downsample: super::downsample::Downsample,
    staging: ID3D11Texture2D,
    input: (u32, u32),
    size: (u32, u32),
}
impl Scale {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        input: (u32, u32),
        size: (u32, u32),
        matrix: u32,
        full_range: bool,
    ) -> Result<Self> {
        ensure!(
            input.0 % 2 == 0 && input.1 % 2 == 0,
            "native preview requires even NV12 dimensions"
        );
        let mut texture = None;
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: input.0,
            Height: input.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut texture))?;
        }
        let input_texture = texture.context("GPU preview NV12 texture")?;
        let downsample = super::downsample::Downsample::new(
            device,
            &input_texture,
            input,
            size,
            matrix,
            full_range,
        )?;
        desc.Width = size.0;
        desc.Height = size.1;
        desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
        }
        Ok(Self {
            context: context.clone(),
            input_texture,
            downsample,
            staging: staging.context("GPU preview staging")?,
            input,
            size,
        })
    }
    pub fn read(&self, texture: &ID3D11Texture2D, subresource: u32) -> Result<Vec<u8>> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut desc);
        }
        ensure!(
            desc.Width >= self.input.0
                && desc.Height >= self.input.1
                && desc.Format == DXGI_FORMAT_NV12
                && desc.MipLevels == 1
                && subresource < desc.ArraySize,
            "native preview decoder output is not supported NV12"
        );
        // Decoder-owned surfaces need not support SRVs. Copy just the visible
        // NV12 rectangle on the GPU, preserving padded surfaces/array slices.
        unsafe {
            self.context.CopySubresourceRegion(
                &self.input_texture,
                0,
                0,
                0,
                0,
                texture,
                subresource,
                Some(&D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: self.input.0,
                    bottom: self.input.1,
                    back: 1,
                }),
            );
        }
        self.downsample.run(&self.context);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .CopyResource(&self.staging, &self.downsample.output);
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }
        let (width, height) = self.size;
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        unsafe {
            for y in 0..height as usize {
                bgra.extend_from_slice(std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    width as usize * 4,
                ));
            }
            self.context.Unmap(&self.staging, 0);
        }
        Ok(bgra)
    }
}
