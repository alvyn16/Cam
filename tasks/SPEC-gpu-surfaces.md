# Spec: gpu-surfaces

Provide an owned render handoff from wgpu's DX12 device to a D3D11 texture on the same physical adapter. Use the raw device's LUID to select D3D11, create an NT-shareable D3D12 BGRA render target, open its handle on D3D11 and import the original D3D12 resource into wgpu. Close every shared handle. Do not reinterpret COM types from different windows crate versions; use the same 0.62 interfaces as wgpu-hal through a named dependency.

Render through an ordinary wgpu encoder, transition the target to COMMON (PRESENT in wgpu), submit and wait for completion before returning the native surface. Keep the wgpu allocation alive for the surface's lifetime. Expose the native texture for the next encoder slice; do not enable the production export path until that encoder is implemented and independently verified.

An explicit hardware test must clear changing colors through wgpu and inspect every resulting pixel through a D3D11 staging texture. CPU readback belongs only in that verification test. Assert native format, size and same-adapter identity, and reject invalid dimensions. Repeat surface creation/drop and check shared-handle counts remain stable. Non-DX12 backends must fail clearly and leave the existing path available.

Commands: `cargo test -j1 --lib render::dx12::tests -- --ignored --test-threads=1 --nocapture`, then normal regression/build checks. No system changes, external publication or unrelated edits. This is the first risk-reduction slice, not complete GPU media parity.
