use anyhow::{Context, Result};
use windows62::{
    Win32::Graphics::{
        Direct3D::{Fxc::D3DCompile, ID3DInclude},
        Direct3D11::*,
        Dxgi::Common::*,
    },
    core::{PCSTR, s},
};
pub(super) struct Downsample {
    pub output: ID3D11Texture2D,
    source: ID3D11ShaderResourceView,
    target: ID3D11UnorderedAccessView,
    shader: ID3D11ComputeShader,
    constants: ID3D11Buffer,
    size: (u32, u32),
}
impl Downsample {
    pub fn new(
        device: &ID3D11Device,
        source: &ID3D11Texture2D,
        input: (u32, u32),
        size: (u32, u32),
    ) -> Result<Self> {
        let code = include_bytes!("downsample.hlsl");
        let (mut compiled, mut error) = (None, None);
        unsafe {
            D3DCompile(
                code.as_ptr().cast(),
                code.len(),
                PCSTR::null(),
                None,
                None::<&ID3DInclude>,
                s!("main"),
                s!("cs_5_0"),
                0,
                0,
                &mut compiled,
                Some(&mut error),
            )
        }
        .context("compiling GPU preview downsample shader")?;
        let compiled = compiled.context("GPU preview shader bytecode")?;
        let bytes = unsafe {
            std::slice::from_raw_parts(
                compiled.GetBufferPointer() as *const u8,
                compiled.GetBufferSize(),
            )
        };
        let mut shader = None;
        unsafe {
            device.CreateComputeShader(bytes, None, Some(&mut shader))?;
        }
        let mut source_view = None;
        unsafe {
            device.CreateShaderResourceView(source, None, Some(&mut source_view))?;
        }
        let mut output = None;
        unsafe {
            device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: size.0,
                    Height: size.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut output),
            )?;
        }
        let output = output.context("GPU downsample output")?;
        let mut target = None;
        unsafe {
            device.CreateUnorderedAccessView(&output, None, Some(&mut target))?;
        }
        let sizes = [input.0, input.1, size.0, size.1];
        let mut constants = None;
        unsafe {
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 16,
                    Usage: D3D11_USAGE_IMMUTABLE,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: sizes.as_ptr().cast(),
                    ..Default::default()
                }),
                Some(&mut constants),
            )?;
        }
        Ok(Self {
            output,
            source: source_view.context("GPU preview SRV")?,
            target: target.context("GPU preview UAV")?,
            shader: shader.context("GPU preview shader")?,
            constants: constants.context("GPU preview dimensions")?,
            size,
        })
    }
    pub fn run(&self, context: &ID3D11DeviceContext) {
        unsafe {
            context.CSSetShader(&self.shader, None);
            context.CSSetShaderResources(0, Some(&[Some(self.source.clone())]));
            context.CSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.CSSetUnorderedAccessViews(
                0,
                1,
                Some([Some(self.target.clone())].as_ptr()),
                None,
            );
            context.Dispatch(self.size.0.div_ceil(8), self.size.1.div_ceil(8), 1);
            context.CSSetShaderResources(0, Some(&[None]));
            context.CSSetUnorderedAccessViews(0, 1, Some([None].as_ptr()), None);
            context.CSSetConstantBuffers(0, Some(&[None]));
            context.CSSetShader(None, None);
        }
    }
}
