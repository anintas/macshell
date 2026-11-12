//! Совместимость с приложением Island (`%LOCALAPPDATA%\Island\island.exe`, Tauri): оно рисует чёлку в центре
//! верхнего края. Когда в его настройках выключено «под панелью задач» (`underTaskbar`), чёлка встаёт на
//! самый верх экрана, поверх нашей панели; в середине панели для неё остаётся место.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::BOOL;

use crate::tray::host::exe_of_pid;

/// Ширина чёлки в покое (логические пиксели). Окно Island шире (684: в нём чёлка раскрывается), но в покое
/// чёлка уже; название приложения слева обрезается до этого места.
pub const NOTCH_PT: f32 = 360.0;

const WINDOW_CLASS: &str = "Tauri Window";
const WINDOW_TITLE: &str = "Island";
const EXE_NAME: &str = "island.exe";

/// Что видно снаружи об окне Island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IslandWindow {
    pub hwnd: isize,
    pub left: i32,
    pub top: i32,
    pub width: i32,
}

struct Search {
    found: Option<IslandWindow>,
}

fn text_of(read: impl Fn(&mut [u16]) -> i32) -> String {
    let mut buf = [0u16; 64];
    let n = read(&mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        if text_of(|b| GetClassNameW(hwnd, b)) != WINDOW_CLASS || text_of(|b| GetWindowTextW(hwnd, b)) != WINDOW_TITLE {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let is_island = exe_of_pid(pid).is_some_and(|p| p.rsplit(['\\', '/']).next().is_some_and(|n| n.eq_ignore_ascii_case(EXE_NAME)));
        if !is_island {
            return true.into();
        }
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_ok() {
            search.found = Some(IslandWindow { hwnd: hwnd.0 as isize, left: r.left, top: r.top, width: r.right - r.left });
            return false.into();
        }
    }
    true.into()
}

/// Окно Island, если приложение запущено.
pub fn find() -> Option<IslandWindow> {
    let mut search = Search { found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as *mut c_void as isize));
    }
    search.found
}

/// Чёлка стоит на верхнем краю экрана, то есть поверх панели, а не под ней.
pub fn sits_in_bar(w: &IslandWindow, bar_height_px: i32) -> bool {
    w.top < bar_height_px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notch_is_in_the_bar_only_when_the_window_starts_above_its_bottom_edge() {
        let at = |top| IslandWindow { hwnd: 1, left: 618, top, width: 684 };
        assert!(sits_in_bar(&at(0), 24));
        assert!(!sits_in_bar(&at(24), 24), "underTaskbar: the window hangs below the bar");
    }

    #[test]
    fn finder_does_not_crash_without_island() {
        // Island может быть запущен или нет: проверяем только, что поиск возвращается.
        let _ = find();
    }
}
