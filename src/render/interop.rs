//! Zero-copy interop between platform video frames and wgpu.

pub struct TextureCache {
    pub device: wgpu::Device,
}

pub struct Wrapped {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
}

impl TextureCache {
    pub fn new(_device: &wgpu::Device) -> Option<Self> {
        None
    }

    pub fn wrap(&self, _pb: &crate::media::PixelBuffer, _usage: wgpu::TextureUsages) -> Option<Wrapped> {
        None
    }

    pub fn flush(&self) {}
}
