#pragma once

// Optional, namespaced caption image published by macshell. If no matching
// mapping exists the original application snapshot is left unchanged.
#include <windows.h>
#include <vector>
#include <cstring>
#include <cstdint>
#include <cwchar>

namespace macshell_caption {
constexpr unsigned kMagic = 0x4D434831u;
constexpr int kMaxWidth = 8192;
constexpr int kMaxHeight = 256;
struct Header { volatile LONG sequence; unsigned magic, width, height; };

inline bool Apply(HWND hwnd, void* destination, int width, int height) {
    if (!hwnd || !destination || width <= 0 || width > kMaxWidth || height <= 0) return false;
    DWORD pid = 0;
    GetWindowThreadProcessId(hwnd, &pid);
    if (!pid) return false;
    wchar_t name[128];
    swprintf(name, 128, L"Local\\macshell.caption.v1.%08lX.%016llX.%d", pid,
             (unsigned long long)(uint32_t)(uintptr_t)hwnd, width);
    HANDLE mapping = OpenFileMappingW(FILE_MAP_READ, FALSE, name);
    if (!mapping) return false;
    const size_t bytes = sizeof(Header) + (size_t)width * kMaxHeight * sizeof(DWORD);
    auto* view = static_cast<const unsigned char*>(MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, bytes));
    if (!view) { CloseHandle(mapping); return false; }
    auto* header = reinterpret_cast<const Header*>(view);
    LONG seq = header->sequence;
    MemoryBarrier();
    bool valid = !(seq & 1) && header->magic == kMagic && header->width == (unsigned)width &&
                 header->height > 0 && header->height <= kMaxHeight && header->height <= (unsigned)height;
    std::vector<DWORD> pixels;
    unsigned rows = 0;
    if (valid) {
        rows = header->height;
        pixels.resize((size_t)width * rows);
        std::memcpy(pixels.data(), view + sizeof(Header), pixels.size() * sizeof(DWORD));
        MemoryBarrier();
        valid = seq == header->sequence;
    }
    if (valid) {
        auto* dst = static_cast<DWORD*>(destination);
        for (size_t i = 0; i < pixels.size(); i++) {
            unsigned alpha = dst[i] >> 24;
            if (!alpha) continue; // preserve rounded, transparent window corners
            DWORD p = pixels[i];
            if (alpha == 255) { dst[i] = p; continue; }
            unsigned r = ((p >> 16) & 255) * alpha / 255;
            unsigned g = ((p >> 8) & 255) * alpha / 255;
            unsigned b = (p & 255) * alpha / 255;
            dst[i] = (alpha << 24) | (r << 16) | (g << 8) | b;
        }
    }
    UnmapViewOfFile(view);
    CloseHandle(mapping);
    return valid;
}
}
