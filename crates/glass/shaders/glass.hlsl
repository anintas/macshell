// Жидкое стекло на D3D11. Формулы повторяют crates/glass/src/cpu.rs.
// Три прохода: горизонтальный блюр, вертикальный блюр, стекло.

cbuffer Params : register(b0)
{
    float2 size;        // ширина, высота в пикселях
    float radius;
    float bevel;
    float refraction;
    float saturation_k;
    float dispersion;
    float rim;
    float4 tint;        // RGB и сила смешивания
    float opacity;
    int blur_radius;    // не больше 16
    int horizontal;     // 1: блюр по строкам, 0: по столбцам
    float pad;
};

Texture2D tex : register(t0);
SamplerState samp : register(s0);

float4 vs_main(uint id : SV_VertexID) : SV_Position
{
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

// Прямоугольное усреднение вдоль одной оси с зажимом по краям (как cpu::box_blur).
float4 ps_blur(float4 pos : SV_Position) : SV_Target
{
    int2 p = int2(pos.xy);
    int2 dim = int2(size);
    int r = min(blur_radius, 16);
    float4 sum = 0;
    for (int i = -r; i <= r; i++)
    {
        int2 q = horizontal != 0 ? int2(p.x + i, p.y) : int2(p.x, p.y + i);
        q = clamp(q, int2(0, 0), dim - 1);
        sum += tex.Load(int3(q, 0));
    }
    return sum / (2 * r + 1);
}

float sdf_rr(float2 p)
{
    float r = clamp(radius, 0.0, min(size.x, size.y) * 0.5);
    float2 q = abs(p - size * 0.5) - (size * 0.5 - r);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

float edge_disp(float sdf)
{
    float d = -sdf;
    if (bevel <= 0.0 || d < 0.0 || d > bevel)
        return 0.0;
    float t = 1.0 - d / bevel;
    return refraction * t * t;
}

float4 ps_glass(float4 pos : SV_Position) : SV_Target
{
    float2 c = pos.xy; // центр пикселя: (x + 0.5, y + 0.5)
    float sdf = sdf_rr(c);
    float coverage = saturate(0.5 - sdf);
    if (coverage <= 0.0)
        return float4(0, 0, 0, 0);

    float2 g = float2(
        sdf_rr(c + float2(0.5, 0.0)) - sdf_rr(c - float2(0.5, 0.0)),
        sdf_rr(c + float2(0.0, 0.5)) - sdf_rr(c - float2(0.0, 0.5)));
    float len = length(g);
    g = len < 1e-6 ? float2(0, 0) : g / len;

    float disp = edge_disp(sdf);
    float2 b = c - g * disp;
    float spread = refraction > 0.0 ? dispersion * disp / refraction : 0.0;
    float3 col;
    col.r = tex.SampleLevel(samp, (b - g * spread) / size, 0).r * 255.0;
    col.g = tex.SampleLevel(samp, b / size, 0).g * 255.0;
    col.b = tex.SampleLevel(samp, (b + g * spread) / size, 0).b * 255.0;

    float lum = dot(col, float3(0.299, 0.587, 0.114));
    col = lum + (col - lum) * saturation_k;
    col = col * (1.0 - tint.a) + tint.rgb * 255.0 * tint.a;

    // Блик: свет сверху слева, как LIGHT в cpu.rs.
    float facing = dot(g, float2(-0.4472136, -0.8944272));
    float k = rim * saturate(1.0 - (-sdf) / 1.5) * min(max(facing, 0.0) + 0.4 * max(-facing, 0.0), 1.0);
    col = col * (1.0 - k) + 255.0 * k;

    float a = coverage * opacity;
    col = clamp(col, 0.0, 255.0) * a / 255.0;
    return float4(col, a);
}
