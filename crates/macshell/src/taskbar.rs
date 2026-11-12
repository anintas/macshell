//! Родная панель задач Windows: прячется, пока работает наш Dock, и возвращается при выходе.
//! Меняется системная настройка автоскрытия, поэтому прежнее состояние записано в файл: если процесс убили,
//! следующий запуск или `macshell --restore-taskbar` вернёт панель.

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{ABM_GETSTATE, ABM_SETSTATE, ABS_ALWAYSONTOP, ABS_AUTOHIDE, APPBARDATA, SHAppBarMessage};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, SW_HIDE, SW_SHOWNA, ShowWindow,
};
use windows::core::BOOL;

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

struct Found {
    own_pid: u32,
    primary: Option<HWND>,
    all: Vec<HWND>,
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let found = unsafe { &mut *(lparam.0 as *mut Found) };
    let class = class_name(hwnd);
    if class == "Shell_TrayWnd" || class == "Shell_SecondaryTrayWnd" {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        // Окно хоста трея с тем же классом принадлежит нам: его трогать нельзя.
        if pid != found.own_pid {
            found.all.push(hwnd);
            if class == "Shell_TrayWnd" {
                found.primary = Some(hwnd);
            }
        }
    }
    true.into()
}

fn find() -> Found {
    let mut found = Found { own_pid: unsafe { GetCurrentProcessId() }, primary: None, all: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut found as *mut Found as isize));
    }
    found
}

fn state_file() -> Option<PathBuf> {
    Some(crate::config::Config::path()?.with_file_name("taskbar-state.txt"))
}

/// Состояние панели без ошибок разбора: только биты автоскрытия и «поверх всех».
pub fn parse_state(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().map(|v| v & (ABS_AUTOHIDE | ABS_ALWAYSONTOP))
}

fn set_state(hwnd: HWND, state: u32) {
    let mut abd = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: hwnd, lParam: LPARAM(state as isize), ..Default::default() };
    unsafe { SHAppBarMessage(ABM_SETSTATE, &mut abd) };
}

fn get_state(hwnd: HWND) -> u32 {
    let mut abd = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: hwnd, ..Default::default() };
    (unsafe { SHAppBarMessage(ABM_GETSTATE, &mut abd) } as u32) & (ABS_AUTOHIDE | ABS_ALWAYSONTOP)
}

/// Возвращает панель задач в состояние, записанное в файле (после аварийного выхода). `true`, если что-то вернули.
pub fn restore_from_file() -> bool {
    let Some(path) = state_file() else { return false };
    let Some(prev) = std::fs::read_to_string(&path).ok().and_then(|t| parse_state(&t)) else { return false };
    let found = find();
    if let Some(primary) = found.primary {
        set_state(primary, prev);
    }
    for hwnd in found.all {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
    }
    let _ = std::fs::remove_file(path);
    true
}

/// Родная панель задач спрятана, пока жив этот объект.
pub struct HiddenTaskbar {
    prev: u32,
    windows: Vec<HWND>,
}

impl HiddenTaskbar {
    pub fn hide() -> Option<HiddenTaskbar> {
        // Если прошлый запуск не вернул панель, настоящее прежнее состояние лежит в файле.
        let saved = state_file().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| parse_state(&t));
        let found = find();
        let primary = found.primary?;
        let prev = saved.unwrap_or_else(|| get_state(primary));
        if let Some(path) = state_file() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, prev.to_string());
        }
        set_state(primary, prev | ABS_AUTOHIDE);
        let hidden = HiddenTaskbar { prev, windows: found.all };
        hidden.enforce();
        Some(hidden)
    }

    /// Проводник мог снова показать панель (клавиша Win, уведомление): прячем ещё раз.
    pub fn enforce(&self) {
        for &hwnd in &self.windows {
            unsafe {
                if IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
    }
}

impl Drop for HiddenTaskbar {
    fn drop(&mut self) {
        let found = find();
        if let Some(primary) = found.primary {
            set_state(primary, self.prev);
        }
        for hwnd in found.all {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNA);
            }
        }
        if let Some(path) = state_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_keeps_only_the_known_bits() {
        assert_eq!(parse_state("1\n"), Some(ABS_AUTOHIDE));
        assert_eq!(parse_state(" 3 "), Some(ABS_AUTOHIDE | ABS_ALWAYSONTOP));
        assert_eq!(parse_state("0"), Some(0));
        assert_eq!(parse_state("255"), Some(ABS_AUTOHIDE | ABS_ALWAYSONTOP));
    }

    #[test]
    fn garbage_in_the_state_file_is_ignored() {
        assert_eq!(parse_state(""), None);
        assert_eq!(parse_state("autohide"), None);
        assert_eq!(parse_state("-1"), None);
    }
}
