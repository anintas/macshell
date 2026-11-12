//! Сквозной тест полосы: нужен рабочий стол, другой экземпляр macshell не запущен.
//! Запуск: `cargo test -p macshell --test bar -- --ignored`.

use std::ffi::c_void;
use std::time::Duration;

use macshell::host::{primary_scale, set_dpi_awareness};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::core::{PCWSTR, w};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SystemParametersInfoW,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

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
fn bar_reserves_work_area_and_quits_on_close() {
    isolate_from_real_tray(true);
    set_dpi_awareness().unwrap();
    let before = work_area_top();
    let handle = std::thread::spawn(macshell::bar::run);
    std::thread::sleep(Duration::from_millis(2000));
    if handle.is_finished() {
        panic!("bar exited early: {:?}", handle.join().unwrap());
    }
    assert!(work_area_top() >= before + 24, "work area top {} before {}", work_area_top(), before);
    assert!(macshell::bar::request_quit(), "bar window not found");
    handle.join().unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(work_area_top(), before);
}

fn layered_windows() -> Vec<HWND> {
    let mut found = Vec::new();
    let mut prev = None;
    unsafe {
        while let Ok(h) = FindWindowExW(None, prev, w!("macshell_layered"), PCWSTR::null()) {
            if h.0.is_null() {
                break;
            }
            found.push(h);
            prev = Some(h);
        }
    }
    found
}

fn lparam(x: i32, y: i32) -> LPARAM {
    LPARAM((((y & 0xFFFF) << 16) | (x & 0xFFFF)) as isize)
}

fn mouse(hwnd: HWND, msg: u32, x: i32, y: i32) {
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(1)), Some(lparam(x, y))) };
}

#[test]
#[ignore]
fn logo_click_opens_menu_and_outside_click_closes_it() {
    isolate_from_real_tray(true);
    set_dpi_awareness().unwrap();
    let scale = primary_scale();
    let handle = std::thread::spawn(macshell::bar::run);
    std::thread::sleep(Duration::from_millis(2000));
    assert_eq!(layered_windows().len(), 1, "only the bar window expected");
    let bar = unsafe { FindWindowW(w!("macshell_layered"), w!("macshell.bar")) }.unwrap();

    let (x, y) = ((20.0 * scale) as i32, (10.0 * scale) as i32);
    for msg in [WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP] {
        mouse(bar, msg, x, y);
    }
    std::thread::sleep(Duration::from_millis(600));
    let windows = layered_windows();
    assert_eq!(windows.len(), 2, "menu window must appear after a logo click");
    let menu = *windows.iter().find(|h| **h != bar).unwrap();

    mouse(menu, WM_LBUTTONDOWN, -5, -5);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(layered_windows().len(), 1, "outside click must close the menu");

    assert!(macshell::bar::request_quit());
    handle.join().unwrap().unwrap();
}

/// Тесты полосы не трогают настоящий трей: хост не занимает уже запущенные приложения.
fn isolate_from_real_tray(tray_off: bool) {
    let path = std::env::temp_dir().join("macshell-test-config.toml");
    std::fs::write(&path, if tray_off { "tray = \"off\"\n" } else { "" }).unwrap();
    unsafe {
        std::env::set_var("MACSHELL_CONFIG", &path);
        std::env::set_var("MACSHELL_NO_CLAIM", "1");
    }
}

#[test]
#[ignore]
fn tray_flyout_lists_probe_icon_and_forwards_click() {
    use capture::RectI;
    use macshell::host::{InputEvent, LayeredWindow, pump_messages};
    use windows::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW};
    use windows::Win32::UI::WindowsAndMessaging::{IDI_INFORMATION, LoadIconW, WM_APP, WM_LBUTTONUP};

    isolate_from_real_tray(false);
    set_dpi_awareness().unwrap();
    let scale = primary_scale();
    let handle = std::thread::spawn(macshell::bar::run);
    std::thread::sleep(Duration::from_millis(2000));
    if handle.is_finished() {
        panic!("bar exited early: {:?}", handle.join().unwrap());
    }

    // Приложение «значок» с обратным вызовом WM_APP + 5.
    let owner = LayeredWindow::new(RectI { x: 300, y: 300, w: 32, h: 32 }).unwrap();
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(owner.hwnd() as *mut _),
        uID: 42,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_APP + 5,
        hIcon: unsafe { LoadIconW(None, IDI_INFORMATION) }.unwrap(),
        ..Default::default()
    };
    for (i, c) in "bar probe".encode_utf16().enumerate() {
        nid.szTip[i] = c;
    }
    assert!(unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool());

    // Открываем окно значков кликами по правой половине полосы, пока оно не появится.
    let bar = unsafe { FindWindowW(w!("macshell_layered"), w!("macshell.bar")) }.unwrap();
    let screen_w = unsafe { windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN) };
    let y = (10.0 * scale) as i32;
    let mut flyout = None;
    let mut x = screen_w - 20;
    while x > screen_w / 2 && flyout.is_none() {
        for msg in [WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP] {
            mouse(bar, msg, x, y);
        }
        std::thread::sleep(Duration::from_millis(120));
        flyout = layered_windows().into_iter().find(|h| *h != bar && h.0 as isize != owner.hwnd());
        x -= 6;
    }
    let flyout = flyout.expect("tray flyout never opened: chevron not found on the bar");

    // Левый клик по первой плитке: приложение получает WM_LBUTTONUP на своё сообщение обратного вызова.
    let (cx, cy) = macshell::tray::tile_center(0);
    let (cx, cy) = ((cx * scale) as i32, (cy * scale) as i32);
    for msg in [WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP] {
        mouse(flyout, msg, cx, cy);
    }
    let end = std::time::Instant::now() + Duration::from_millis(1000);
    let mut got_up = false;
    let mut seen: Vec<InputEvent> = Vec::new();
    while std::time::Instant::now() < end && !got_up {
        pump_messages(30);
        let events = owner.take_input();
        seen.extend(events.iter().copied());
        got_up = events.into_iter().any(|ev| {
            matches!(ev, InputEvent::Message { msg, wparam, lparam } if msg == WM_APP + 5 && wparam == 42 && lparam == WM_LBUTTONUP as isize)
        });
    }
    assert!(got_up, "icon owner never received WM_LBUTTONUP on its callback message; saw {seen:?}; windows {}", layered_windows().len());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(layered_windows().len(), 2, "flyout must close after a click (bar and the probe owner remain)");

    unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
    assert!(macshell::bar::request_quit());
    handle.join().unwrap().unwrap();
}

#[test]
#[ignore]
fn bar_goes_solid_when_a_window_is_maximized() {
    use macshell::host::pump_messages;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SW_SHOWMAXIMIZED, ShowWindow, WNDCLASSW, WS_EX_APPWINDOW,
        WS_OVERLAPPEDWINDOW,
    };

    unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> windows::Win32::Foundation::LRESULT {
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    isolate_from_real_tray(true);
    set_dpi_awareness().unwrap();
    let handle = std::thread::spawn(macshell::bar::run);
    std::thread::sleep(Duration::from_millis(2000));
    if handle.is_finished() {
        panic!("bar exited early: {:?}", handle.join().unwrap());
    }
    let before = macshell::bar::is_covered_state();

    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: hinst.into(), lpszClassName: w!("macshell_bar_cover_probe"), ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(WS_EX_APPWINDOW, w!("macshell_bar_cover_probe"), w!("probe"), WS_OVERLAPPEDWINDOW, 100, 100, 400, 300, None, None, Some(hinst.into()), None).unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
        let end = std::time::Instant::now() + Duration::from_millis(1500);
        while std::time::Instant::now() < end {
            pump_messages(20);
        }
        assert!(macshell::bar::is_covered_state(), "развёрнутое окно должно сделать панель сплошной");

        let _ = DestroyWindow(hwnd);
        let end = std::time::Instant::now() + Duration::from_millis(1500);
        while std::time::Instant::now() < end {
            pump_messages(20);
        }
    }
    // Если до теста на ПК уже было развёрнутое окно, возвращаться некуда: проверяем только возврат прежнего состояния.
    assert_eq!(macshell::bar::is_covered_state(), before, "после закрытия окна состояние должно вернуться");

    assert!(macshell::bar::request_quit());
    handle.join().unwrap().unwrap();
}
