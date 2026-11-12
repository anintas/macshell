//! Читает окна системы для адаптивной панели: закрывает ли какое-то окно рабочий стол. Решает
//! `bar_style::desktop_covered`, здесь только чтение окон и события, по которым состояние надо пересчитать.

use std::ffi::c_void;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use capture::RectI;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND, EnumWindows,
    GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsIconic, IsWindowVisible,
    PostMessageW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW, WINEVENT_OUTOFCONTEXT,
    WS_EX_TOOLWINDOW, WS_MAXIMIZE,
};
use windows::core::BOOL;

use crate::bar_style::{WinInfo, desktop_covered, is_click_through};

/// Классы окон оболочки: рабочий стол, панели задач, значки в углу, наши окна. Это не приложения.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "NotifyIconOverflowWindow",
    "macshell_layered",
];

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lparam.0 as *mut Vec<WinInfo>) };
    unsafe {
        // Невидимые и свёрнутые отсеиваем до остальных вызовов: их большинство.
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let minimized = IsIconic(hwnd).as_bool();
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return true.into();
        }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
        out.push(WinInfo {
            rect: RectI { x: rect.left, y: rect.top, w: rect.right - rect.left, h: rect.bottom - rect.top },
            maximized: style & WS_MAXIMIZE.0 != 0,
            minimized,
            visible: true,
            cloaked: cloaked != 0,
            tool_window: ex_style & WS_EX_TOOLWINDOW.0 != 0,
            click_through: is_click_through(ex_style),
            shell_class: SHELL_CLASSES.contains(&class_name(hwnd).as_str()),
        });
    }
    true.into()
}

/// Видимые окна верхнего уровня.
pub fn snapshot() -> Vec<WinInfo> {
    let mut out: Vec<WinInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut out as *mut Vec<WinInfo> as isize));
    }
    out
}

/// Рабочая область основного монитора (без места, занятого панелями).
pub fn work_area() -> RectI {
    let mut r = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut r as *mut RECT as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    RectI { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

pub fn is_desktop_covered() -> bool {
    desktop_covered(&snapshot(), work_area())
}

static NOTIFY_HWND: AtomicIsize = AtomicIsize::new(0);
static NOTIFY_MSG: AtomicU32 = AtomicU32::new(0);
/// Сообщение уже отправлено и ещё не обработано: лишние события не плодят новых.
static PENDING: AtomicBool = AtomicBool::new(false);
static HOOKS: Once = Once::new();

const OBJID_WINDOW: i32 = 0;

unsafe extern "system" fn on_event(_h: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, _child: i32, _thread: u32, _time: u32) {
    let relevant = match event {
        EVENT_SYSTEM_MOVESIZEEND | EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND => true,
        // Развёртывание и возврат окна: изменилось положение активного окна.
        EVENT_OBJECT_LOCATIONCHANGE => id_object == OBJID_WINDOW && hwnd == unsafe { GetForegroundWindow() },
        _ => false,
    };
    if !relevant || PENDING.swap(true, Ordering::Relaxed) {
        return;
    }
    let target = NOTIFY_HWND.load(Ordering::Relaxed);
    if target != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(target as *mut _)), NOTIFY_MSG.load(Ordering::Relaxed), WPARAM(0), LPARAM(0));
        }
    }
}

/// Окну `hwnd` приходит сообщение `msg`, когда стоит пересчитать состояние («закрыт» или «рабочий стол»).
/// Смену активного окна сообщает `foreground`. Нужен цикл сообщений в этом потоке.
pub fn watch(hwnd: isize, msg: u32) {
    NOTIFY_HWND.store(hwnd, Ordering::Relaxed);
    NOTIFY_MSG.store(msg, Ordering::Relaxed);
    HOOKS.call_once(|| unsafe {
        SetWinEventHook(EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MINIMIZEEND, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
    });
}

/// Сообщение обработано: следующее событие снова разбудит панель.
pub fn ack() {
    PENDING.store(false, Ordering::Relaxed);
}
