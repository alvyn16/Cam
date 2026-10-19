Texture2D<float> luma : register(t0);
Texture2D<float2> chroma : register(t1);
SamplerState chroma_sampler : register(s0);
RWTexture2D<float4> output_image : register(u0);
cbuffer Sizes : register(b0) { uint2 source_size; uint2 output_size; uint yuv_matrix; uint full_range; uint2 padding; };

float3 rgb_at(uint2 pixel) {
    float y = luma.Load(int3(pixel, 0));
    // NV12 uses horizontally left-sited, vertically centered chroma.
    float2 uv = chroma.SampleLevel(chroma_sampler, (float2(pixel) + float2(1.0, 0.5)) / float2(source_size), 0) - 128.0 / 255.0;
    if (full_range == 0) {
        y = (y - 16.0 / 255.0) * (255.0 / 219.0);
        uv *= 255.0 / 224.0;
    }
    float3 rgb = yuv_matrix == 1
        ? float3(y + 1.5748 * uv.y, y - 0.187324 * uv.x - 0.468124 * uv.y, y + 1.8556 * uv.x)
        : float3(y + 1.402 * uv.y, y - 0.344136 * uv.x - 0.714136 * uv.y, y + 1.772 * uv.x);
    // Match conversion before area filtering, including clipping at RGB gamut.
    return saturate(rgb);
}

// Integrate covered source pixels instead of point sampling fine 4K text and
// patterns. Work is bounded by source pixels, and never leaves GPU memory.
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= output_size)) return;
    float2 low = float2(id.xy) * float2(source_size) / float2(output_size);
    float2 high = float2(id.xy + 1) * float2(source_size) / float2(output_size);
    uint2 first = uint2(floor(low));
    uint2 last = min(uint2(ceil(high)), source_size);
    float3 sum = 0;
    float weight_sum = 0;
    for (uint y = first.y; y < last.y; ++y) {
        float wy = max(0.0, min(high.y, float(y + 1)) - max(low.y, float(y)));
        for (uint x = first.x; x < last.x; ++x) {
            float wx = max(0.0, min(high.x, float(x + 1)) - max(low.x, float(x)));
            float weight = wx * wy;
            sum += rgb_at(uint2(x, y)) * weight;
            weight_sum += weight;
        }
    }
    // Store BGRA bytes in the RGBA UAV so readback needs no CPU swizzle.
    output_image[id.xy] = float4((sum / max(weight_sum, 1e-6)).bgr, 1.0);
}
