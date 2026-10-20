# Shared GPU editor previews

Hand the reduced native preview to the compositor in shared GPU textures. Keep
the existing antialiased NV12 conversion, 1280-pixel cap, latest-request mailbox,
color handling, cancellation, offline full-resolution rendering and readback
fallback. This change does not add adaptive preview quality or GPU export decode.

Use the compositor's DX12 adapter for D3D11 decoding. A bounded four-surface
pool per source owns the shared allocations. Native completion must precede
publication; compositor completion and COMMON-state handoff must precede reuse.
Never infer GPU completion merely from dropping a CPU frame or counting frames.
Unsupported backends/devices retain native readback/FFmpeg compatibility.

Diagnostics: CAM_GPU_PREVIEW_SHARED=off selects the existing readback route;
require rejects missing shared-frame support. Compare the same source, requested
times, output dimensions and executable across modes. Verify final rendered
pixels, sequential playback, superseded seeks, cancellation, resource bounds,
fallback, teardown and actual editor rendering. Record CPU/wall/GPU allocation
measurements, and inspect a captured editor image before reporting completion.
