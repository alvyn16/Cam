//! Zero-copy interop between CoreVideo and wgpu: decoded video frames and
//! encoder pixel buffers are IOSurface-backed, so they can be used as
//! Metal textures directly instead of being copied through the CPU.

use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_video::*;
use objc2_metal::{MTLPixelFormat, MTLTexture, MTLTextureType};

use crate::util::SendBox;

pub struct TextureCache {
    cache: SendBox<CFRetained<CVMetalTextureCache>>,
    device: wgpu::Device,
}

/// A pixel buffer viewed as a wgpu texture. Keeps the buffer alive.
pub struct Wrapped {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
    _cv: SendBox<CFRetained<CVMetalTexture>>,
    pub pb: SendBox<CFRetained<CVPixelBuffer>>,
}

impl TextureCache {
    /// None if the device isn't Metal (then callers fall back to copies).
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        let mtl: Retained<ProtocolObject<dyn objc2_metal::MTLDevice>> = {
            let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }?;
            hal.raw_device().clone()
        };
        let mut out = std::ptr::null_mut();
        let st = unsafe { CVMetalTextureCache::create(None, None, &mtl, None, NonNull::from(&mut out)) };
        if st != 0 || out.is_null() {
            return None;
        }
        let cache = unsafe { CFRetained::from_raw(NonNull::new(out)?) };
        Some(TextureCache { cache: SendBox(cache), device: device.clone() })
    }

    /// Wrap a BGRA pixel buffer as a texture with the given usage.
    pub fn wrap(&self, pb: &CVPixelBuffer, usage: wgpu::TextureUsages) -> Option<Wrapped> {
        let (w, h) = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb));
        let mut tex = std::ptr::null_mut();
        let st = unsafe {
            CVMetalTextureCache::create_texture_from_image(
                None,
                &self.cache.0,
                pb,
                None,
                MTLPixelFormat::BGRA8Unorm,
                w,
                h,
                0,
                NonNull::from(&mut tex),
            )
        };
        if st != 0 || tex.is_null() {
            return None;
        }
        let cv = unsafe { CFRetained::from_raw(NonNull::new(tex)?) };
        let mtl: Retained<ProtocolObject<dyn MTLTexture>> = CVMetalTextureGetTexture(&cv)?;
        let size = wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 };
        let hal = unsafe {
            wgpu::hal::metal::Device::texture_from_raw(
                mtl,
                wgpu::TextureFormat::Bgra8Unorm,
                MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent { width: w as u32, height: h as u32, depth: 1 },
                None,
            )
        };
        let texture = unsafe {
            self.device.create_texture_from_hal::<wgpu::hal::api::Metal>(
                hal,
                &wgpu::TextureDescriptor {
                    label: Some("corevideo"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage,
                    view_formats: &[],
                },
                wgpu::TextureUses::UNINITIALIZED,
            )
        };
        let view = texture.create_view(&Default::default());
        let pb = unsafe { CFRetained::retain(NonNull::from(pb)) };
        Some(Wrapped { texture, view, size: (w as u32, h as u32), _cv: SendBox(cv), pb: SendBox(pb) })
    }

    /// Release cached textures no longer in use.
    pub fn flush(&self) {
        self.cache.0.flush(0);
    }
}
