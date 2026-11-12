//! Интерактивные тесты: нужен рабочий стол. Запуск: `cargo test -p capture -- --ignored --test-threads=1`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use capture::{RectI, capture_region, capture_region_raw, exclude_from_capture, forget_window};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::CreateSolidBrush;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

static CLASS_COUNTER: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn pump_for(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Окно без рамки поверх всех, закрашенное сплошным цветом. `color_bgr` — значение `COLORREF` (0x00BBGGRR).
fn spawn_solid_window(rect: RectI, color_bgr: u32) -> isize {
    unsafe {
        let n = CLASS_COUNTER.fetch_add(1, Ordering::SeqCst);
        let name: Vec<u16> = format!("capture_test_{n}\0").encode_utf16().collect();
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: PCWSTR(name.as_ptr()),
            hbrBackground: CreateSolidBrush(COLORREF(color_bgr)),
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&wc), 0);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(name.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        pump_for(200);
        hwnd.0 as isize
    }
}

fn destroy(hwnd: isize) {
    unsafe {
        let _ = DestroyWindow(HWND(hwnd as *mut _));
    }
}

fn center(b: &glass::Bitmap) -> u32 {
    b.pixels[(b.height / 2 * b.width + b.width / 2) as usize]
}

const RED_COLORREF: u32 = 0x0000FF;
const RECT: RectI = RectI { x: 200, y: 200, w: 64, h: 64 };

#[test]
#[ignore]
fn captures_solid_red_window() {
    let hwnd = spawn_solid_window(RECT, RED_COLORREF);
    let bmp = capture_region(RECT).unwrap();
    destroy(hwnd);
    assert_eq!(center(&bmp), 0xFFFF0000);
}

#[test]
#[ignore]
fn registered_window_is_hidden_from_backdrop_capture_but_not_from_screenshots() {
    let hwnd = spawn_solid_window(RECT, RED_COLORREF);
    exclude_from_capture(hwnd).unwrap();
    pump_for(200);
    for _ in 0..5 {
        assert_ne!(center(&capture_region(RECT).unwrap()), 0xFFFF0000);
        assert_eq!(center(&capture_region_raw(RECT).unwrap()), 0xFFFF0000);
    }
    forget_window(hwnd);
    destroy(hwnd);
}

#[test]
#[ignore]
fn off_screen_region_does_not_fail() {
    let bmp = capture_region(RectI { x: -50, y: -50, w: 100, h: 100 }).unwrap();
    assert_eq!((bmp.width, bmp.height), (100, 100));
    assert_eq!(bmp.pixels[0], 0xFF000000);
}

#[test]
#[ignore]
fn size_matches_request() {
    let bmp = capture_region(RectI { x: 10, y: 10, w: 120, h: 30 }).unwrap();
    assert_eq!((bmp.width, bmp.height), (120, 30));
    assert_eq!(bmp.pixels.len(), 3600);
}
