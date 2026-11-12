//! Откуда «выезжает» окно приложения при восстановлении из свёрнутого состояния. Нужен рабочий стол.
//! Запуск: `cargo test -p macshell --test animation_origin -- --ignored --nocapture --test-threads=1`.
//! Пишет координаты окна на первых кадрах анимации: у родной анимации это точка у кнопки на панели задач.

use std::time::{Duration, Instant};

use capture::{RectI, capture_region};
use macshell::host::{pump_messages, set_dpi_awareness};
use macshell::tray::host::TrayHost;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::CreateSolidBrush;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_MINIMIZE,
    SW_RESTORE, SW_SHOWNORMAL, ShowWindow, WNDCLASSW, WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCWSTR, w};

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn pump_for(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        pump_messages(10);
    }
}

/// Рамка пикселей цвета окна (пурпурный) на кадре, шаг выборки 4 пикселя.
fn magenta_box(frame: &glass::Bitmap) -> Option<(i32, i32, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut found = false;
    for y in (0..frame.height).step_by(4) {
        for x in (0..frame.width).step_by(4) {
            if frame.pixels[(y * frame.width + x) as usize] & 0x00FFFFFF == 0x00FF00FF {
                found = true;
                x0 = x0.min(x as i32);
                y0 = y0.min(y as i32);
                x1 = x1.max(x as i32);
                y1 = y1.max(y as i32);
            }
        }
    }
    found.then_some((x0, y0, x1, y1))
}

/// Первые кадры восстановления окна из свёрнутого состояния: (миллисекунды, рамка окна).
fn restore_frames(class: PCWSTR) -> Vec<(u128, (i32, i32, i32, i32))> {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hbrBackground: CreateSolidBrush(COLORREF(0x00FF00FF)),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class,
            w!("animation origin probe"),
            WS_OVERLAPPEDWINDOW,
            700,
            300,
            500,
            400,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        pump_for(600);
        let _ = ShowWindow(hwnd, SW_MINIMIZE);
        pump_for(900);

        let screen = RectI { x: 0, y: 0, w: GetSystemMetrics(SM_CXSCREEN), h: GetSystemMetrics(SM_CYSCREEN) };
        let start = Instant::now();
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let mut frames = Vec::new();
        while start.elapsed() < Duration::from_millis(700) {
            if let Ok(frame) = capture_region(screen) {
                if let Some(b) = magenta_box(&frame) {
                    frames.push((start.elapsed().as_millis(), b));
                }
            }
            pump_messages(1);
        }
        let _ = DestroyWindow(hwnd);
        pump_for(300);
        frames
    }
}

fn describe(label: &str, frames: &[(u128, (i32, i32, i32, i32))]) {
    println!("--- {label}: {} кадров с окном", frames.len());
    for (t, b) in frames.iter().take(6) {
        println!("  {t:>4} мс  окно x {}..{}  y {}..{}  (центр {}, {})", b.0, b.2, b.1, b.3, (b.0 + b.2) / 2, (b.1 + b.3) / 2);
    }
}

/// Окно должно появляться снизу, у родной панели задач, а не у верхнего края.
fn assert_starts_from_the_bottom(label: &str, frames: &[(u128, (i32, i32, i32, i32))]) {
    describe(label, frames);
    let first = frames.first().unwrap_or_else(|| panic!("{label}: окно не попало ни на один кадр"));
    let center_y = (first.1.1 + first.1.3) / 2;
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    assert!(center_y > screen_h / 2, "{label}: анимация начинается не снизу, а на y={center_y} из {screen_h}");
}

#[test]
#[ignore]
fn restore_animation_starts_from_the_taskbar_with_the_tray_host_and_with_the_bar() {
    set_dpi_awareness().unwrap();
    assert_starts_from_the_bottom("без хоста трея", &restore_frames(w!("macshell_anim_probe_a")));

    let host = TrayHost::start(0, 0, false).unwrap();
    assert_starts_from_the_bottom("с хостом трея", &restore_frames(w!("macshell_anim_probe_b")));
    drop(host);

    // Целая панель: окно AppBar сверху и хост трея внутри. Не трогаем уже запущенные приложения.
    let path = std::env::temp_dir().join("macshell-anim-config.toml");
    std::fs::write(&path, "").unwrap();
    unsafe {
        std::env::set_var("MACSHELL_CONFIG", &path);
        std::env::set_var("MACSHELL_NO_CLAIM", "1");
    }
    let bar = std::thread::spawn(macshell::bar::run);
    std::thread::sleep(Duration::from_millis(2000));
    assert!(!bar.is_finished(), "панель не запустилась");
    assert_starts_from_the_bottom("с запущенной панелью", &restore_frames(w!("macshell_anim_probe_c")));
    assert!(macshell::bar::request_quit());
    bar.join().unwrap().unwrap();
}
