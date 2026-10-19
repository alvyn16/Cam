Texture2D<float4> source_image : register(t0);
RWTexture2D<float4> output_image : register(u0);
cbuffer Sizes : register(b0) { uint2 source_size; uint2 output_size; };

// Integrate covered source pixels instead of point sampling fine 4K text and
// patterns. Work is bounded by source pixels, and never leaves GPU memory.
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= output_size)) return;
    float2 low = float2(id.xy) * float2(source_size) / float2(output_size);
    float2 high = float2(id.xy + 1) * float2(source_size) / float2(output_size);
    uint2 first = uint2(floor(low));
    uint2 last = min(uint2(ceil(high)), source_size);
    float4 sum = 0;
    float weight_sum = 0;
    for (uint y = first.y; y < last.y; ++y) {
        float wy = max(0.0, min(high.y, float(y + 1)) - max(low.y, float(y)));
        for (uint x = first.x; x < last.x; ++x) {
            float wx = max(0.0, min(high.x, float(x + 1)) - max(low.x, float(x)));
            float weight = wx * wy;
            sum += source_image.Load(int3(x, y, 0)) * weight;
            weight_sum += weight;
        }
    }
    output_image[id.xy] = sum / max(weight_sum, 1e-6);
}
