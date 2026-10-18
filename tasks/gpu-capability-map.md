# Native Windows GPU media pipeline

The existing FFmpeg/raw-pixel path is the working fallback. It does not satisfy Glide's GPU media interoperability. Implement and independently verify these capabilities in order; do not select the native path merely because a texture can be allocated.

| Module | Contract | Depends on |
|---|---|---|
| gpu-surfaces | Render wgpu/D3D12 into owned BGRA surfaces opened on a matching D3D11 device, with explicit resource-state transition and completion | wgpu HAL, DXGI adapter LUID |
| gpu-encoder | Encode those D3D11 surfaces with a hardware Media Foundation transform, honoring format/rate/bitrate, bounded in-flight work and errors | gpu-surfaces |
| gpu-export | Connect compositing to encoder without readback; keep audio mux, cancellation, atomic output and fallback behavior | gpu-encoder, existing export/audio |
| gpu-decoder | Decode standard screen/camera media to GPU surfaces, import/convert without CPU pixel upload, preserve seek/timestamps | gpu-surfaces, Media Foundation source reader |
| gpu-capture | Transfer WGC surfaces to native encoder without CPU readback, preserve region/window/timing/camera behavior | gpu-encoder, existing WGC |

No completion claim until a real current-app recording/export uses the native route and its media independently decodes with changing pixels, correct colors/timing/audio, pause/seek/cancellation and bounded resource use. ProRes/software fallback must continue working. Test adapter mismatch/unsupported formats and device loss. Direct USB iPhone capture remains a separate capability and needs a connected supported device for validation.

References: [D3D11-aware transforms](https://learn.microsoft.com/en-us/windows/win32/medfound/mf-sa-d3d11-aware), [D3D12 video encoding](https://microsoft.github.io/DirectX-Specs/d3d/D3D12_Video_Encoding_H264_HEVC.html), and the installed wgpu-hal 30.0.1 DX12 source. A CPU wait for GPU completion is allowed; copying pixel bytes through CPU memory is not a GPU-to-encoder route.
