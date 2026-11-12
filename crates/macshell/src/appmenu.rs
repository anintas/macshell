//! Меню активного приложения в верхней полосе, как в macOS: названия меню окна (`HMENU`) рядом с названием
//! приложения и пункты «Скрыть», «Скрыть остальные», «Завершить». Работает с классическими окнами Win32: у
//! Electron, UWP и приложений на WinUI меню не `HMENU`, их прочитать нельзя, и у них в полосе только название.

use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetMenu, GetMenuItemCount, GetMenuItemInfoW, GetWindowThreadProcessId, HMENU, IsIconic,
    MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED, MFS_GRAYED, MFT_SEPARATOR, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU,
    PostMessageW, SMTO_ABORTIFHUNG, SW_MINIMIZE, SendMessageTimeoutW, ShowWindow, WM_CLOSE, WM_COMMAND, WM_INITMENUPOPUP,
};
use windows::core::PWSTR;

use crate::winlist;

/// Сколько названий меню читается из окна: у больших программ их бывает больше, чем помещается в полосе.
pub const MAX_TITLES: usize = 128;
/// Глубина вложенных подменю, которая разворачивается в один список.
const MAX_DEPTH: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// Команда для `WM_COMMAND`; 0 у разделителей и заголовков вложенных меню.
    pub command: u32,
    pub enabled: bool,
    pub checked: bool,
    pub separator: bool,
}

/// «&Файл» дают «Файл», «Save &As...\tCtrl+S» дают «Save As...»: быстрая клавиша `&` и подсказка после табуляции
/// убираются, `&&` остаётся знаком `&`.
pub fn clean_label(raw: &str) -> String {
    let text = raw.split('\t').next().unwrap_or("");
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                out.push('&');
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

/// Окно, чьё меню и приложение показывает полоса: активное, но не рабочий стол и не окно самой оболочки.
pub fn front_window() -> Option<isize> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let class = class_name(hwnd);
    (!matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "macshell_layered")).then_some(hwnd.0 as isize)
}

pub fn pid_of(hwnd: isize) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
    pid
}

struct Raw {
    label: String,
    id: u32,
    sub: HMENU,
    state: u32,
    separator: bool,
}

fn read_item(menu: HMENU, position: u32) -> Option<Raw> {
    let mut text = [0u16; 256];
    let mut info = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STRING | MIIM_ID | MIIM_SUBMENU | MIIM_STATE | MIIM_FTYPE,
        dwTypeData: PWSTR(text.as_mut_ptr()),
        cch: text.len() as u32 - 1,
        ..Default::default()
    };
    unsafe { GetMenuItemInfoW(menu, position, true, &mut info) }.ok()?;
    let len = (info.cch as usize).min(text.len() - 1);
    Some(Raw {
        label: String::from_utf16_lossy(&text[..len]),
        id: info.wID,
        sub: info.hSubMenu,
        state: info.fState.0,
        separator: info.fType.0 & MFT_SEPARATOR.0 != 0,
    })
}

fn count(menu: HMENU) -> u32 {
    let n = unsafe { GetMenuItemCount(Some(menu)) };
    n.max(0) as u32
}

fn menu_of(hwnd: isize) -> Option<HMENU> {
    let menu = unsafe { GetMenu(HWND(hwnd as *mut _)) };
    (!menu.0.is_null()).then_some(menu)
}

/// Названия меню окна слева направо. Пусто, если у окна нет `HMENU`.
pub fn titles(hwnd: isize) -> Vec<String> {
    let Some(menu) = menu_of(hwnd) else { return Vec::new() };
    (0..count(menu).min(MAX_TITLES as u32))
        .filter_map(|i| read_item(menu, i))
        .filter(|r| !r.separator)
        .map(|r| clean_label(&r.label))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Просит окно подготовить подменю (многие приложения включают и отмечают пункты именно в этот момент).
fn prepare(hwnd: isize, sub: HMENU, position: u32) {
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND(hwnd as *mut _),
            WM_INITMENUPOPUP,
            WPARAM(sub.0 as usize),
            LPARAM(position as isize),
            SMTO_ABORTIFHUNG,
            150,
            None,
        );
    }
}

fn collect(hwnd: isize, menu: HMENU, depth: usize, out: &mut Vec<Item>, max: usize) {
    for i in 0..count(menu) {
        if out.len() >= max {
            return;
        }
        let Some(raw) = read_item(menu, i) else { continue };
        if raw.separator {
            if !out.is_empty() && !out.last().is_some_and(|l| l.separator) {
                out.push(Item { label: String::new(), command: 0, enabled: false, checked: false, separator: true });
            }
            continue;
        }
        let label = clean_label(&raw.label);
        if label.is_empty() {
            continue;
        }
        if !raw.sub.0.is_null() {
            // Вложенное меню: заголовок серым и его пункты ниже с отступом, пока не кончится допустимая глубина.
            if depth + 1 < MAX_DEPTH {
                out.push(Item { label: format!("{label} ›"), command: 0, enabled: false, checked: false, separator: false });
                prepare(hwnd, raw.sub, i);
                let start = out.len();
                collect(hwnd, raw.sub, depth + 1, out, max);
                for item in &mut out[start..] {
                    if !item.separator {
                        item.label = format!("    {}", item.label);
                    }
                }
            } else {
                out.push(Item { label: format!("{label} ›"), command: 0, enabled: false, checked: false, separator: false });
            }
            continue;
        }
        let blocked = raw.state & (MFS_GRAYED.0 | MFS_DISABLED.0) != 0;
        out.push(Item { label, command: raw.id, enabled: !blocked, checked: raw.state & MFS_CHECKED.0 != 0, separator: false });
    }
}

/// Пункты `index`-го меню окна одним списком (не больше `max`), вложенные меню развёрнуты с отступом.
pub fn items(hwnd: isize, index: usize, max: usize) -> Vec<Item> {
    let Some(menu) = menu_of(hwnd) else { return Vec::new() };
    let Some((position, top)) = (0..count(menu).min(MAX_TITLES as u32))
        .filter_map(|position| read_item(menu, position).map(|item| (position, item)))
        .filter(|(_, item)| !item.separator && !clean_label(&item.label).is_empty())
        .nth(index) else { return Vec::new() };
    let mut out = Vec::new();
    if !top.sub.0.is_null() {
        prepare(hwnd, top.sub, position);
        collect(hwnd, top.sub, 0, &mut out, max);
    }
    while out.last().is_some_and(|l| l.separator) {
        out.pop();
    }
    out
}

/// Выполняет пункт меню в окне приложения.
pub fn invoke(hwnd: isize, command: u32) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_COMMAND, WPARAM(command as usize), LPARAM(0));
    }
}

/// Окна приложения с этим процессом: (окно, процесс).
fn app_windows() -> Vec<(isize, u32)> {
    winlist::list().into_iter().map(|w| (w.hwnd, pid_of(w.hwnd))).collect()
}

/// Какие из окон нужно свернуть при «Скрыть остальные»: все, кроме окон процесса `keep`.
pub fn others_of(windows: &[(isize, u32)], keep: u32) -> Vec<isize> {
    windows.iter().filter(|(_, pid)| *pid != keep).map(|(h, _)| *h).collect()
}

fn minimize(windows: &[isize]) {
    for h in windows {
        let hwnd = HWND(*h as *mut _);
        unsafe {
            if !IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            }
        }
    }
}

/// «Скрыть»: сворачивает окна приложения.
pub fn hide_app(pid: u32) {
    let mine: Vec<isize> = app_windows().into_iter().filter(|(_, p)| *p == pid).map(|(h, _)| h).collect();
    minimize(&mine);
}

/// «Скрыть остальные»: сворачивает окна всех приложений, кроме этого.
pub fn hide_others(pid: u32) {
    minimize(&others_of(&app_windows(), pid));
}

/// «Завершить»: просит закрыться каждое окно приложения, как это делает кнопка закрытия (несохранённое
/// приложение спросит само).
pub fn quit_app(pid: u32) {
    for (h, p) in app_windows() {
        if p == pid {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreateMenu, CreatePopupMenu, CreateWindowExW, DestroyWindow, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR,
        MF_STRING, WINDOW_EX_STYLE, WS_POPUP,
    };
    use windows::core::{PCWSTR, w};

    #[test]
    fn mnemonics_and_accelerators_are_stripped() {
        assert_eq!(clean_label("&Файл"), "Файл");
        assert_eq!(clean_label("Save &As...\tCtrl+Shift+S"), "Save As...");
        assert_eq!(clean_label("Tom && Jerry"), "Tom & Jerry");
        assert_eq!(clean_label("  &  "), "");
    }

    #[test]
    fn hide_others_keeps_the_front_process() {
        let windows = [(1, 10), (2, 20), (3, 10), (4, 30)];
        assert_eq!(others_of(&windows, 10), vec![2, 4]);
        assert!(others_of(&windows, 99).len() == 4);
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Окно без рамки с меню: File (New, разделитель, Exit серым, вложенное More) и Edit.
    #[test]
    fn reads_titles_items_checks_and_nested_menus_from_a_real_window() {
        unsafe {
            let bar = CreateMenu().unwrap();
            let file = CreatePopupMenu().unwrap();
            let more = CreatePopupMenu().unwrap();
            let edit = CreatePopupMenu().unwrap();
            let add = |m: HMENU, flags, id: usize, text: &str| {
                let t = wide(text);
                AppendMenuW(m, flags, id, PCWSTR(t.as_ptr())).unwrap();
            };
            add(file, MF_STRING | MF_CHECKED, 101, "&New\tCtrl+N");
            AppendMenuW(file, MF_SEPARATOR, 0, PCWSTR::null()).unwrap();
            add(file, MF_STRING | MF_GRAYED, 102, "E&xit");
            add(more, MF_STRING, 301, "Deep");
            add(file, MF_POPUP, more.0 as usize, "More");
            add(edit, MF_STRING, 201, "&Copy");
            add(bar, MF_POPUP, file.0 as usize, "&File");
            add(bar, MF_POPUP, edit.0 as usize, "&Edit");
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("menu test"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                Some(bar),
                None,
                None,
            )
            .unwrap();
            let h = hwnd.0 as isize;
            assert_eq!(titles(h), vec!["File", "Edit"]);
            let file_items = items(h, 0, 30);
            let labels: Vec<&str> = file_items.iter().map(|i| i.label.as_str()).collect();
            assert_eq!(labels, vec!["New", "", "Exit", "More ›", "    Deep"]);
            assert!(file_items[0].checked && file_items[0].enabled && file_items[0].command == 101);
            assert!(file_items[1].separator);
            assert!(!file_items[2].enabled, "grayed item stays disabled");
            assert_eq!(file_items[4].command, 301);
            assert_eq!(items(h, 1, 30).len(), 1);
            assert_eq!(items(h, 0, 3).len(), 3, "list is capped");
            assert_eq!(items(h, 0, 2).len(), 1, "a trailing separator is dropped");
            assert!(items(h, 7, 30).is_empty(), "missing menu gives nothing");
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn window_without_a_menu_has_no_titles() {
        let hwnd = unsafe { GetForegroundWindow() };
        // Окно вне нашего процесса без HMENU или без окна вовсе: ничего не падает.
        let _ = titles(hwnd.0 as isize);
        assert!(titles(0).is_empty());
    }
}
