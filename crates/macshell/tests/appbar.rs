//! Интерактивный тест: нужен рабочий стол. Запуск: `cargo test -p macshell --test appbar -- --ignored`.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use capture::RectI;
use macshell::appbar::AppBar;
use macshell::host::{LayeredWindow, pump_messages, set_dpi_awareness};
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW, WM_APP,
};

fn work_area_top() -> i32 {
    let mut r = RECT::default();
    unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut r as *mut RECT as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
            .unwrap();
    }
    r.top
}

fn settle() {
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        pump_messages(20);
    }
}

#[test]
#[ignore]
fn reserves_top_strip() {
    set_dpi_awareness().unwrap();
    let before = work_area_top();
    let win = LayeredWindow::new(RectI { x: 0, y: 0, w: 800, h: 28 }).unwrap();
    let bar = AppBar::register(win.hwnd(), WM_APP + 1, 28).unwrap();
    settle();
    assert!(work_area_top() >= before + 28, "work area top {} before {}", work_area_top(), before);
    assert_eq!(bar.rect().h, 28);
    drop(bar);
    settle();
    assert_eq!(work_area_top(), before);
}
