//! Explicit GPU preview scaling/color conversion. Untagged legacy recordings
//! use FFmpeg's BT.601 default instead of Media Foundation's HD heuristic.
use anyhow::{Context, Result, ensure};
use windows62::{
    Win32::{
        Foundation::RECT,
        Graphics::{Direct3D11::*, Dxgi::Common::*},
    },
    core::Interface,
};
pub(super) struct Scale {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    video: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output: ID3D11Texture2D,
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
        fps: u32,
        matrix: u32,
        full_range: bool,
    ) -> Result<Self> {
        let video: ID3D11VideoDevice = device.cast()?;
        let video_context: ID3D11VideoContext = context.cast()?;
        let rate = DXGI_RATIONAL {
            Numerator: fps,
            Denominator: 1,
        };
        let enumerator = unsafe {
            video.CreateVideoProcessorEnumerator(&D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: input.0,
                InputHeight: input.1,
                OutputFrameRate: rate,
                OutputWidth: input.0,
                OutputHeight: input.1,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            })?
        };
        let processor = unsafe { video.CreateVideoProcessor(&enumerator, 0)? };
        let color: ID3D11VideoContext1 = video_context.cast()?;
        let space = match (matrix, full_range) {
            (1, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            (1, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
            (_, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
            (_, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
        };
        unsafe {
            color.VideoProcessorSetStreamColorSpace1(&processor, 0, space);
            color.VideoProcessorSetOutputColorSpace1(
                &processor,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            );
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            video_context.VideoProcessorSetStreamSourceRect(
                &processor,
                0,
                true,
                Some(&RECT {
                    left: 0,
                    top: 0,
                    right: input.0 as i32,
                    bottom: input.1 as i32,
                }),
            );
            video_context.VideoProcessorSetStreamDestRect(
                &processor,
                0,
                true,
                Some(&RECT {
                    left: 0,
                    top: 0,
                    right: input.0 as i32,
                    bottom: input.1 as i32,
                }),
            );
            video_context.VideoProcessorSetOutputTargetRect(
                &processor,
                true,
                Some(&RECT {
                    left: 0,
                    top: 0,
                    right: input.0 as i32,
                    bottom: input.1 as i32,
                }),
            );
        }
        let mut output = None;
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: input.0,
            Height: input.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            ..Default::default()
        };
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut output))?;
        }
        let output = output.context("GPU preview output")?;
        let downsample = super::downsample::Downsample::new(device, &output, input, size)?;
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
            _device: device.clone(),
            context: context.clone(),
            video,
            video_context,
            enumerator,
            processor,
            output,
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
                && desc.Format == DXGI_FORMAT_NV12,
            "native preview decoder output is not supported NV12"
        );
        let (mut input_view, mut output_view) = (None, None);
        unsafe {
            self.video.CreateVideoProcessorInputView(
                texture,
                &self.enumerator,
                &D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    FourCC: 0,
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPIV {
                            MipSlice: 0,
                            ArraySlice: subresource,
                        },
                    },
                },
                Some(&mut input_view),
            )?;
            self.video.CreateVideoProcessorOutputView(
                &self.output,
                &self.enumerator,
                &D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                },
                Some(&mut output_view),
            )?;
        }
        let output_view = output_view.context("GPU preview output view")?;
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(
                input_view.context("GPU preview input view")?,
            )),
            ..Default::default()
        };
        let result = unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                &output_view,
                0,
                std::slice::from_ref(&stream),
            )
        };
        unsafe {
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
        }
        result.context("GPU preview scale and color conversion")?;
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
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Ok(bgra)
    }
}
