//! Окна, которые панель задач показывала бы кнопками: видимые, без владельца, не скрытые DWM. Для каждого
//! определяется приложение: AUMID окна (упакованные приложения и те, что задали его сами) или путь exe.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

use crate::dock::{AppWindow, key_of, target_for_aumid};
use crate::tray::host::exe_of_pid;

/// Классы окон оболочки и служебные окна: в Dock им не место.
const SKIP_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "NotifyIconOverflowWindow",
    "macshell_layered",
    "Windows.UI.Core.CoreWindow",
];

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

/// Нужна ли кнопка окну с такими признаками (правила панели задач).
pub fn wants_button(visible: bool, cloaked: bool, has_owner: bool, ex_style: u32, has_title: bool, skipped_class: bool) -> bool {
    if !visible || cloaked || skipped_class || !has_title {
        return false;
    }
    let tool = ex_style & WS_EX_TOOLWINDOW.0 != 0;
    let app = ex_style & WS_EX_APPWINDOW.0 != 0;
    let no_activate = ex_style & WS_EX_NOACTIVATE.0 != 0;
    if no_activate && !app {
        return false;
    }
    // Окно с владельцем (диалог) кнопки не получает, если не потребовало её само.
    (!has_owner && !tool) || app
}

/// AUMID окна из его хранилища свойств, если приложение его задало.
fn aumid_of(hwnd: HWND) -> Option<String> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let text = PropVariantToStringAlloc(&value as *const _).ok()?;
        let s = text.to_string().ok().filter(|s| !s.is_empty());
        CoTaskMemFree(Some(text.0 as *const c_void));
        s
    }
}

struct Collect {
    own_pid: u32,
    out: Vec<AppWindow>,
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let acc = unsafe { &mut *(lparam.0 as *mut Collect) };
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let has_owner = GetWindow(hwnd, GW_OWNER).is_ok_and(|h| !h.0.is_null());
        let text = title(hwnd);
        let class = class_name(hwnd);
        let skipped = SKIP_CLASSES.contains(&class.as_str());
        if !wants_button(true, cloaked != 0, has_owner, ex_style, !text.is_empty(), skipped) {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == acc.own_pid {
            return true.into();
        }
        let (key, target) = match aumid_of(hwnd) {
            Some(aumid) => (key_of(&aumid), target_for_aumid(&aumid)),
            None => match exe_of_pid(pid) {
                Some(exe) => (key_of(&exe), exe),
                None => return true.into(),
            },
        };
        acc.out.push(AppWindow { hwnd: hwnd.0 as isize, key, target, title: text });
    }
    true.into()
}

/// Окна верхнего уровня в порядке Z (сверху вниз), которым положена кнопка.
pub fn list() -> Vec<AppWindow> {
    let mut acc = Collect { own_pid: unsafe { GetCurrentProcessId() }, out: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut acc as *mut Collect as isize));
    }
    acc.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(bits: &[u32]) -> u32 {
        bits.iter().fold(0, |a, b| a | b)
    }

    #[test]
    fn plain_visible_titled_window_gets_a_button() {
        assert!(wants_button(true, false, false, 0, true, false));
    }

    #[test]
    fn hidden_cloaked_untitled_or_shell_windows_do_not() {
        assert!(!wants_button(false, false, false, 0, true, false));
        assert!(!wants_button(true, true, false, 0, true, false));
        assert!(!wants_button(true, false, false, 0, false, false));
        assert!(!wants_button(true, false, false, 0, true, true));
    }

    #[test]
    fn tool_windows_and_owned_dialogs_are_skipped_unless_they_ask_for_a_button() {
        assert!(!wants_button(true, false, false, ex(&[WS_EX_TOOLWINDOW.0]), true, false));
        assert!(!wants_button(true, false, true, 0, true, false));
        assert!(wants_button(true, false, true, ex(&[WS_EX_APPWINDOW.0]), true, false));
    }

    #[test]
    fn non_activating_overlays_are_skipped() {
        assert!(!wants_button(true, false, false, ex(&[WS_EX_NOACTIVATE.0]), true, false));
    }

    #[test]
    #[ignore]
    fn list_real_windows() {
        for w in list() {
            println!("{:>10} {:<40} {}", w.hwnd, w.key, w.title);
        }
    }
}
