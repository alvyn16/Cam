//! GPU video-processor conversion; no CPU mapping or pixel readback.
use crate::render::dx12::{GpuInterop, NativeFrame};
use anyhow::{Context, Result, ensure};
use windows62::{
    Win32::Graphics::{Direct3D10::ID3D10Multithread, Direct3D11::*, Dxgi::Common::*},
    core::Interface,
};

pub(super) struct Converter {
    video: ID3D11VideoDevice,
    context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    size: (u32, u32),
}
impl Converter {
    pub fn new(interop: &GpuInterop, width: u32, height: u32, fps: u32) -> Result<Self> {
        ensure!(
            width > 0 && height > 0 && width % 2 == 0 && height % 2 == 0 && fps > 0,
            "native NV12 encoding requires positive even dimensions and frame rate"
        );
        unsafe {
            let _ = interop
                .context()
                .cast::<ID3D10Multithread>()?
                .SetMultithreadProtected(true);
        }
        let video: ID3D11VideoDevice = interop.device().cast()?;
        let context: ID3D11VideoContext = interop.context().cast()?;
        let rate = DXGI_RATIONAL {
            Numerator: fps,
            Denominator: 1,
        };
        let enumerator = unsafe {
            video.CreateVideoProcessorEnumerator(&D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: width,
                InputHeight: height,
                OutputFrameRate: rate,
                OutputWidth: width,
                OutputHeight: height,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            })?
        };
        let input = unsafe { enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM)? };
        let output = unsafe { enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12)? };
        ensure!(
            input & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32 != 0
                && output & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 != 0,
            "GPU video processor cannot convert BGRA to NV12"
        );
        let processor = unsafe { video.CreateVideoProcessor(&enumerator, 0)? };
        let color: ID3D11VideoContext1 = context.cast()?;
        unsafe {
            color.VideoProcessorSetStreamColorSpace1(
                &processor,
                0,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            );
            color.VideoProcessorSetOutputColorSpace1(
                &processor,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            );
            context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
        }
        Ok(Self {
            video,
            context,
            enumerator,
            processor,
            size: (width, height),
        })
    }
    #[cfg(test)]
    pub fn convert(
        &self,
        interop: &GpuInterop,
        frame: &NativeFrame,
        index: u32,
    ) -> Result<ID3D11Texture2D> {
        ensure!(
            frame.size() == self.size,
            "GPU encoder frame dimensions changed"
        );
        let mut output = None;
        unsafe {
            interop.device().CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: self.size.0,
                    Height: self.size.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut output),
            )?;
        }
        let output = output.context("allocating GPU encoder NV12 surface")?;
        self.convert_into(frame, &output, 0, index)?;
        unsafe {
            interop.context().Flush();
        }
        Ok(output)
    }

    pub fn convert_into(
        &self,
        frame: &NativeFrame,
        output: &ID3D11Texture2D,
        subresource: u32,
        index: u32,
    ) -> Result<()> {
        ensure!(
            frame.size() == self.size,
            "GPU encoder frame dimensions changed"
        );
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            output.GetDesc(&mut desc);
        }
        ensure!(
            desc.Format == DXGI_FORMAT_NV12
                && desc.Width >= self.size.0
                && desc.Height >= self.size.1
                && desc.MipLevels == 1
                && subresource < desc.ArraySize,
            "invalid pooled NV12 encoder texture"
        );
        let output_desc = if desc.ArraySize > 1 {
            D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2DARRAY,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2DArray: D3D11_TEX2D_ARRAY_VPOV {
                        MipSlice: 0,
                        FirstArraySlice: subresource,
                        ArraySize: 1,
                    },
                },
            }
        } else {
            D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                ..Default::default()
            }
        };
        let (mut input_view, mut output_view) = (None, None);
        unsafe {
            self.video.CreateVideoProcessorInputView(
                frame.texture(),
                &self.enumerator,
                &D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    FourCC: 0,
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                },
                Some(&mut input_view),
            )?;
            self.video.CreateVideoProcessorOutputView(
                output,
                &self.enumerator,
                &output_desc,
                Some(&mut output_view),
            )?;
        }
        let input_view = input_view.context("creating BGRA input view")?;
        let output_view = output_view.context("creating NV12 output view")?;
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            InputFrameOrField: index,
            pInputSurface: std::mem::ManuallyDrop::new(Some(input_view)),
            ..Default::default()
        };
        let result = unsafe {
            self.context.VideoProcessorBlt(
                &self.processor,
                &output_view,
                index,
                std::slice::from_ref(&stream),
            )
        };
        unsafe {
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
        }
        result.context("GPU BGRA to NV12 conversion")?;
        Ok(())
    }
}
