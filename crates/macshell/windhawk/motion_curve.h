#pragma once

#include <algorithm>
#include <cmath>

namespace dock_motion {

struct Geometry { float x, y, width, height; };
struct Point { float x, y; };

inline float smooth(float value) {
    const float t = std::clamp(value, 0.0f, 1.0f);
    return t * t * (3.0f - 2.0f * t);
}

// Rows nearer the Dock lead the fold; the opposite edge follows, producing
// the characteristic Genie silhouette while vertical rows remain ordered.
inline float genie_row(float progress, float row, bool dock_above) {
    constexpr float spread = 0.55f;
    const float v = dock_above ? (1.0f - row) : row;
    return smooth(progress * (1.0f + spread) - (1.0f - v) * spread);
}

// Genie lamp curve, adapted from the MIT-licensed MacOS Minimize Animation
// by Abdullah Masood and Potassiumuncher. Coordinates are physical pixels.
inline Point genie(float tx, float ty, float progress, const Geometry& window, const Geometry& icon) {
    constexpr float pi = 3.14159265f;
    const float p = std::clamp(progress, 0.0f, 1.0f);
    const float split = 0.30f;
    const float k = (p <= split) ? p / split : 1.0f;
    const float j = (p > split) ? (p - split) / (1.0f - split) : 0.0f;
    const float extra = icon.y - window.y - window.height;
    const float full = (icon.y - window.y) - extra * (1.0f - k);
    const float height = full * (1.0f - j);
    const float y = ty * height;
    const float x = tx * icon.width + tx * (window.width - icon.width) * (1.0f - j) * (1.0f - ty)
                  + tx * (window.width - icon.width) * (1.0f - k) * ty;
    const float safe_full = std::fabs(full) < 0.001f ? (full < 0.0f ? -0.001f : 0.001f) : full;
    const float offset_x = (icon.x - window.x) * (y / safe_full) * k + (icon.x - window.x) * j;
    const float offset_y = icon.y - window.y - height - extra * (1.0f - k);
    const float sway = std::sin(((height - y) / safe_full) * 2.0f * pi + pi)
                     * (window.x + window.width * tx - (icon.x + icon.width * tx)) / 7.0f * k;
    return {window.x + x + offset_x + sway, window.y + y + offset_y};
}

inline float opacity(float progress) {
    return smooth((1.0f - std::clamp(progress, 0.0f, 1.0f)) / 0.12f);
}

} // namespace dock_motion
