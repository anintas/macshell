#include "../motion_curve.h"
#include <cassert>
#include <cmath>

int main() {
    using namespace dock_motion;
    const Geometry win{100.0f, 80.0f, 900.0f, 700.0f};
    const Geometry icon{1320.0f, 1032.0f, 48.0f, 48.0f};
    assert(opacity(0.0f) == 1.0f && opacity(1.0f) == 0.0f);
    assert(genie_row(0.5f, 1.0f, false) - genie_row(0.5f, 0.0f, false) > 0.5f);
    assert(genie_row(0.5f, 0.0f, true) - genie_row(0.5f, 1.0f, true) > 0.5f);
    for (float row : {0.0f, 0.25f, 0.5f, 0.75f, 1.0f}) {
        assert(genie_row(0.0f, row, false) == 0.0f);
        assert(genie_row(1.0f, row, false) == 1.0f);
        float previous = 0.0f;
        for (int step = 0; step <= 100; ++step) {
            float current = genie_row(step / 100.0f, row, false);
            assert(current >= previous);
            previous = current;
        }
    }
    for (float tx : {0.0f, 0.25f, 0.5f, 0.75f, 1.0f}) {
        for (float ty : {0.0f, 0.25f, 0.5f, 0.75f, 1.0f}) {
            Point start = genie(tx, ty, 0.0f, win, icon);
            assert(std::fabs(start.x - (win.x + tx * win.width)) < 0.01f);
            assert(std::fabs(start.y - (win.y + ty * win.height)) < 0.01f);
            Point end = genie(tx, ty, 1.0f, win, icon);
            assert(std::fabs(end.x - (icon.x + tx * icon.width)) < 0.01f);
            assert(std::fabs(end.y - icon.y) < 0.01f);
        }
    }
    // A Genie curve must actually bend, not be a plain scale or skew.
    Point top = genie(0.0f, 0.0f, 0.5f, win, icon);
    Point middle = genie(0.0f, 0.5f, 0.5f, win, icon);
    Point bottom = genie(0.0f, 1.0f, 0.5f, win, icon);
    assert(std::fabs(middle.x - (top.x + bottom.x) / 2.0f) > 1.0f);
    for (float y : {-50.0f, 80.0f, 430.0f, 780.0f, 1032.0f}) {
        const Geometry near_icon{1320.0f, y, 48.0f, 48.0f};
        for (int step = 0; step <= 100; ++step) {
            for (int row = 0; row <= 40; ++row) {
                Point p = genie(0.5f, row / 40.0f, step / 100.0f, win, near_icon);
                assert(std::isfinite(p.x) && std::isfinite(p.y));
            }
        }
    }
}
