//! Интерактивные тесты чтения окон системы: нужен рабочий стол. Запуск: `cargo test -p macshell --test cover -- --ignored`.

use std::time::{Duration, Instant};

use macshell::cover::{snapshot, work_area};
use macshell::host::{pump_messages, set_dpi_awareness};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SW_SHOWMAXIMIZED, ShowWindow, WNDCLASSW, WS_EX_APPWINDOW,
    WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCWSTR, w};

fn pump_for(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        pump_messages(20);
    }
}

#[test]
#[ignore]
fn own_maximized_window_is_seen_as_covering_and_shell_windows_are_flagged() {
    set_dpi_awareness().unwrap();
    let work = work_area();
    assert!(work.w > 0 && work.h > 0);

    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("macshell_cover_test"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd: HWND = CreateWindowExW(
            WS_EX_APPWINDOW,
            w!("macshell_cover_test"),
            w!("cover test"),
            WS_OVERLAPPEDWINDOW,
            100,
            100,
            400,
            300,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
        pump_for(400);

        let all = snapshot();
        let mine = all.iter().find(|w| w.maximized && macshell::bar_style::covers(w, work));
        assert!(mine.is_some(), "развёрнутое окно теста должно быть среди покрывающих: {all:#?}");
        assert!(macshell::bar_style::desktop_covered(&all, work));
        assert!(all.iter().any(|w| w.shell_class), "рабочий стол или панель задач помечены как оболочка");

        let _ = DestroyWindow(hwnd);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

#[allow(dead_code)]
const _UNUSED: Option<PCWSTR> = None;
