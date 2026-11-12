#include "../caption_snapshot.h"
#include <cassert>
#include <cstdlib>

int main(int argc, char** argv) {
    if (argc == 2) {
        auto hwnd = (HWND)(uintptr_t)std::strtoull(argv[1], nullptr, 10);
        DWORD dst[32]; for (auto& p : dst) p = 0xff000000u;
        dst[0] = 0; dst[1] = 0x80101010u;
        assert(macshell_caption::Apply(hwnd, dst, 8, 4));
        assert(dst[0] == 0);
        assert(dst[1] == 0x80081119u);
        assert(dst[2] == 0xff112233u);
        assert(dst[16] == 0xff000000u);
        return 0;
    }
    HWND hwnd = CreateWindowExW(0, L"STATIC", L"caption mapping test", WS_POPUP,
                                0, 0, 100, 100, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
    assert(hwnd);
    DWORD pid = GetCurrentProcessId();
    wchar_t name[128];
    swprintf(name, 128, L"Local\\macshell.caption.v1.%08lX.%016llX.%d", pid,
             (unsigned long long)(uint32_t)(uintptr_t)hwnd, 8);
    const size_t bytes = sizeof(macshell_caption::Header) + 8 * macshell_caption::kMaxHeight * sizeof(DWORD);
    HANDLE map = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0, (DWORD)bytes, name);
    assert(map);
    auto* data = static_cast<unsigned char*>(MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, bytes));
    assert(data);
    auto* header = reinterpret_cast<macshell_caption::Header*>(data);
    header->sequence = 2; header->magic = macshell_caption::kMagic; header->width = 8; header->height = 2;
    auto* pixels = reinterpret_cast<DWORD*>(data + sizeof(*header));
    for (int i=0;i<16;i++) pixels[i] = 0xff112233u;
    DWORD dst[32]; for (auto& p : dst) p = 0xff000000u;
    dst[0] = 0; dst[1] = 0x80101010u;
    assert(macshell_caption::Apply(hwnd, dst, 8, 4));
    assert(dst[0] == 0 && dst[1] == 0x80081119u && dst[2] == 0xff112233u && dst[16] == 0xff000000u);
    header->sequence = 3;
    assert(!macshell_caption::Apply(hwnd, dst, 8, 4));
    UnmapViewOfFile(data); CloseHandle(map); DestroyWindow(hwnd);
}
