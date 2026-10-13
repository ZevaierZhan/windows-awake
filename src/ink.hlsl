// Original ink mask + density, sampled from adjacent continuous source frames.
Texture2DArray<float2> ink : register(t0);
SamplerState linearBorder : register(s0);
cbuffer State : register(b0) {
    float4 timing; // progress, opacity, width / height, padding
    float4 fraction; // fraction for each corner's frame pair
};
struct Vertex { float4 position : SV_POSITION; float2 uv : TEXCOORD0; };
Vertex vs(uint id : SV_VertexID) {
    Vertex v;
    v.uv = float2((id << 1) & 2, id & 2);
    v.position = float4(v.uv * float2(2, -2) + float2(-1, 1), 0, 1);
    return v;
}
float4 ps(Vertex v) : SV_TARGET {
    float p = timing.x;
    float4 result = 0;
    for (uint c = 0; c < 4; ++c) {
        float2 q = float2((c & 1) ? 1-v.uv.x : v.uv.x,
                         (c & 2) ? 1-v.uv.y : v.uv.y);
        // Rotate the source's downward flow towards the diagonal from each corner.
        float2 uv = float2(0.50 + (q.x-q.y)*0.63, (q.x+q.y)*0.51);
        float2 a = ink.Sample(linearBorder, float3(uv, c*2));
        float2 b = ink.Sample(linearBorder, float3(uv, c*2+1));
        float2 sample = lerp(a, b, fraction[c]);
        // Advance a soft spatial frontier over the full configured duration.
        // The original fast pour is the texture detail inside that moving frontier.
        float distance = length(q*float2(timing.z,1)) / (0.5*sqrt(timing.z*timing.z+1));
        float edge = 1-smoothstep(p*1.15-0.10,p*1.15+0.10,distance);
        float alpha = sample.x*edge;
        float density = saturate(sample.y / 0.32);
        float3 pigment = lerp(float3(0.010,0.033,0.041), float3(0.060,0.190,0.210), density);
        float4 plume = float4(pigment*alpha, alpha);
        result = plume + result*(1-alpha);
    }
    // Apply global opacity once so four overlapping corners do not darken too early.
    result *= timing.y;
    // Finish continuously with a dark veil, closing gaps before the hold phase.
    float finish = smoothstep(0.86, 1.0, p);
    float3 behind = float3(0.010,0.025,0.030);
    float4 opaque = float4(result.rgb + behind*(1-result.a),1);
    return lerp(result, opaque, finish);
}
