//! Интерактивные тесты хоста трея: нужен рабочий стол. Запуск: `cargo test -p macshell --test tray -- --ignored --test-threads=1`.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use capture::RectI;
use macshell::appbar::AppBar;
use macshell::host::{LayeredWindow, pump_messages, set_dpi_awareness};
use macshell::tray::host::TrayHost;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::{
    IDI_INFORMATION, LoadIconW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW, WM_APP,
};

fn settle() {
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        pump_messages(20);
    }
}

fn nid(owner: &LayeredWindow, tip: &str) -> NOTIFYICONDATAW {
    let mut n = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(owner.hwnd() as *mut _),
        uID: 42,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_APP + 5,
        hIcon: unsafe { LoadIconW(None, IDI_INFORMATION) }.unwrap(),
        ..Default::default()
    };
    for (i, c) in tip.encode_utf16().take(127).enumerate() {
        n.szTip[i] = c;
    }
    n
}

#[test]
#[ignore]
fn own_notify_icon_reaches_host() {
    set_dpi_awareness().unwrap();
    let host = TrayHost::start(0, 0, false).unwrap();
    let owner = LayeredWindow::new(RectI { x: 100, y: 100, w: 32, h: 32 }).unwrap();
    let mut n = nid(&owner, "macshell test icon");
    assert!(unsafe { Shell_NotifyIconW(NIM_ADD, &n) }.as_bool(), "NIM_ADD refused");
    let icons = host.icons();
    assert_eq!(icons.len(), 1, "icon must reach our Shell_TrayWnd");
    assert_eq!(icons[0].tip, "macshell test icon");
    assert_eq!(icons[0].callback_msg, WM_APP + 5);

    for (i, c) in "second".encode_utf16().chain([0]).enumerate() {
        n.szTip[i] = c;
    }
    assert!(unsafe { Shell_NotifyIconW(NIM_MODIFY, &n) }.as_bool());
    assert_eq!(host.icons()[0].tip, "second");

    assert!(unsafe { Shell_NotifyIconW(NIM_DELETE, &n) }.as_bool());
    assert!(host.icons().is_empty());
}

fn work_area_top() -> i32 {
    let mut r = RECT::default();
    unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut r as *mut RECT as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
            .unwrap();
    }
    r.top
}

#[test]
#[ignore]
fn appbar_still_works_with_host() {
    set_dpi_awareness().unwrap();
    let before = work_area_top();
    let host = TrayHost::start(0, 0, false).unwrap();
    let win = LayeredWindow::new(RectI { x: 0, y: 0, w: 800, h: 28 }).unwrap();
    let bar = AppBar::register(win.hwnd(), WM_APP + 1, 28).unwrap();
    settle();
    let end = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < end && work_area_top() < before + 28 {
        pump_messages(50);
    }
    assert!(work_area_top() >= before + 28, "appbar message must reach Explorer through the host: top {} before {}", work_area_top(), before);
    drop(bar);
    settle();
    assert_eq!(work_area_top(), before);
    drop(host);
}
