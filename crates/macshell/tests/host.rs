//! Интерактивные тесты: нужен рабочий стол. Запуск: `cargo test -p macshell -- --ignored --test-threads=1`.

use capture::RectI;
use macshell::host::{LayeredWindow, set_dpi_awareness};
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetThreadDpiAwarenessContext,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindowVisible, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

const RECT: RectI = RectI { x: 100, y: 100, w: 64, h: 64 };

#[test]
#[ignore]
fn window_has_required_styles() {
    let w = LayeredWindow::new(RECT).unwrap();
    let ex = w.ex_style();
    for bit in [WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_NOACTIVATE] {
        assert_eq!(ex & bit.0, bit.0, "missing ex-style bit {:#x}", bit.0);
    }
}

#[test]
#[ignore]
fn window_is_visible_after_creation() {
    let w = LayeredWindow::new(RECT).unwrap();
    assert!(unsafe { IsWindowVisible(HWND(w.hwnd() as *mut _)) }.as_bool());
}

#[test]
#[ignore]
fn mouse_click_reaches_queue() {
    use macshell::host::InputEvent;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_LBUTTONDOWN};

    let w = LayeredWindow::new(RECT).unwrap();
    let lparam = LPARAM(((5 << 16) | 10) as isize);
    unsafe { SendMessageW(HWND(w.hwnd() as *mut _), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lparam)) };
    assert_eq!(w.take_input(), vec![InputEvent::Down { x: 10, y: 5 }]);
    assert!(w.take_input().is_empty());
}

#[test]
#[ignore]
fn right_click_reaches_queue() {
    use macshell::host::InputEvent;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_RBUTTONUP};

    let w = LayeredWindow::new(RECT).unwrap();
    let lparam = LPARAM(((7 << 16) | 3) as isize);
    unsafe { SendMessageW(HWND(w.hwnd() as *mut _), WM_RBUTTONUP, Some(WPARAM(0)), Some(lparam)) };
    assert_eq!(w.take_input(), vec![InputEvent::RightUp { x: 3, y: 7 }]);
}

#[test]
#[ignore]
fn dpi_awareness_is_per_monitor_v2() {
    set_dpi_awareness().unwrap();
    let current = unsafe { GetThreadDpiAwarenessContext() };
    let same = unsafe { AreDpiAwarenessContextsEqual(current, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(same.as_bool());
}

#[test]
#[ignore]
fn present_accepts_bitmap_of_window_size() {
    let w = LayeredWindow::new(RECT).unwrap();
    let mut red = glass::Bitmap::new(64, 64);
    red.pixels.fill(0xFFFF0000);
    assert!(w.present(&red).is_ok());
    assert!(w.present(&glass::Bitmap::new(32, 32)).is_err());
}

#[test]
#[ignore]
fn blend_and_offset_are_accepted_and_remembered() {
    let w = LayeredWindow::new(RECT).unwrap();
    let mut red = glass::Bitmap::new(64, 64);
    red.pixels.fill(0xFFFF0000);
    w.present(&red).unwrap();
    assert_eq!(w.blend(), (255, 0));
    w.set_blend(100, -6).unwrap();
    assert_eq!(w.blend(), (100, -6));
    // Обычное обновление кадра не сбрасывает анимацию.
    w.present(&red).unwrap();
    assert_eq!(w.blend(), (100, -6));
    assert_eq!(w.rect(), RECT, "положение окна для вызывающего не меняется");
}

#[test]
#[ignore]
fn window_sets_the_arrow_cursor_instead_of_keeping_the_busy_one() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursor, HTCLIENT, IDC_ARROW, IDC_WAIT, LoadCursorW, SendMessageW, SetCursor, WM_MOUSEMOVE, WM_SETCURSOR,
    };

    let w = LayeredWindow::new(RECT).unwrap();
    unsafe {
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap();
        SetCursor(Some(LoadCursorW(None, IDC_WAIT).unwrap())); // как «загрузка» над панелью у владельца
        let lparam = LPARAM(((WM_MOUSEMOVE << 16) | HTCLIENT) as isize);
        SendMessageW(HWND(w.hwnd() as *mut _), WM_SETCURSOR, Some(WPARAM(w.hwnd() as usize)), Some(lparam));
        assert_eq!(GetCursor(), arrow, "над окном должна быть обычная стрелка");
    }
}
