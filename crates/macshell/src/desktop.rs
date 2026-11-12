//! «Показать рабочий стол» без побочных эффектов: окна приложений сворачиваются по одному и запоминаются, второй
//! вызов разворачивает те же окна. Системное «свернуть всё» (`Shell.MinimizeAll`) прячет и окна macshell, поэтому
//! здесь трогаются только окна, которым положена кнопка панели задач.

use std::sync::Mutex;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, SW_MINIMIZE, SW_SHOWNOACTIVATE, ShowWindow};

static MINIMIZED: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// Окна, которые нужно свернуть: ещё не свёрнутые.
pub fn to_minimize(windows: &[(isize, bool)]) -> Vec<isize> {
    windows.iter().filter(|(_, iconic)| !iconic).map(|(h, _)| *h).collect()
}

/// Свернуть все окна приложений или, если уже сворачивали, вернуть их. `true`, если окна свёрнуты.
pub fn toggle() -> bool {
    let mut saved = MINIMIZED.lock().unwrap_or_else(|e| e.into_inner());
    if !saved.is_empty() {
        for h in saved.drain(..).rev() {
            let hwnd = HWND(h as *mut _);
            unsafe {
                if IsWindow(Some(hwnd)).as_bool() {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
            }
        }
        return false;
    }
    let state: Vec<(isize, bool)> =
        crate::winlist::list().iter().map(|w| (w.hwnd, unsafe { IsIconic(HWND(w.hwnd as *mut _)) }.as_bool())).collect();
    for h in to_minimize(&state) {
        unsafe {
            let _ = ShowWindow(HWND(h as *mut _), SW_MINIMIZE);
        }
        saved.push(h);
    }
    !saved.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_minimized_windows_are_left_alone_and_not_restored_later() {
        assert_eq!(to_minimize(&[(1, false), (2, true), (3, false)]), vec![1, 3]);
        assert!(to_minimize(&[]).is_empty());
    }
}
