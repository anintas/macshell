//! Приложения, чьи значки остались в родном трее Проводника (не ответили на `TaskbarCreated`: запущены от
//! администратора или не слушают это сообщение). Список берётся из реестра `NotifyIconSettings` и списка
//! запущенных процессов. Живого значка и родного меню у них нет: клик показывает окно приложения или
//! запускает его, поэтому в окне значков они показаны приглушёнными.

use std::collections::HashSet;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::UI::Shell::{FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOWNORMAL,
    SetForegroundWindow, ShowWindow,
};
use windows::core::{GUID, PCWSTR, w};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FallbackApp {
    pub exe_path: String,
    /// Подсказка значка из реестра (может быть пустой).
    pub tip: String,
}

/// Имя файла в нижнем регистре: так сравниваем программы, не завися от пути.
pub fn file_name_lower(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

/// Из записей реестра (путь, подсказка) оставляет запущенные приложения без живого значка в хосте,
/// по одной записи на файл. Проводник пропускается: его значки системные.
pub fn select_fallbacks(
    entries: &[(String, String)],
    running: &HashSet<String>,
    hosted: &HashSet<String>,
) -> Vec<FallbackApp> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (path, tip) in entries {
        let name = file_name_lower(path);
        if name.is_empty() || name == "explorer.exe" || !running.contains(&name) || hosted.contains(&name) {
            continue;
        }
        if seen.insert(name) {
            out.push(FallbackApp { exe_path: path.clone(), tip: tip.clone() });
        }
    }
    out
}

/// Windows хранит путь значка с идентификатором известной папки вместо `C:\Program Files`:
/// `{7C5A40EF-...}\Steam\steam.exe`. Подставляет настоящий путь, остальные пути не трогает.
pub fn resolve_placeholder(path: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let Some(rest) = path.strip_prefix('{') else { return path.to_string() };
    let Some((guid, tail)) = rest.split_once('}') else { return path.to_string() };
    match lookup(guid) {
        Some(folder) => format!("{}{}", folder.trim_end_matches('\\'), tail),
        None => path.to_string(),
    }
}

fn known_folder(guid: &str) -> Option<String> {
    let id = GUID::try_from(guid).ok()?;
    // В реестре встречаются только эти две; остальные пути идут без подстановки.
    if id != FOLDERID_ProgramFilesX86 && id != FOLDERID_ProgramFiles {
        return None;
    }
    unsafe {
        let p = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn reg_string(key: HKEY, sub: &str, value: PCWSTR) -> Option<String> {
    let sub: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(key, PCWSTR(sub.as_ptr()), value, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut bytes))
            .ok()
            .ok()?;
    }
    let units = (bytes as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..units]))
}

/// Все записи `HKCU\Control Panel\NotifyIconSettings`: (путь к exe, подсказка).
pub fn registry_entries() -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        let mut root = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, w!("Control Panel\\NotifyIconSettings"), None, KEY_READ, &mut root).is_err() {
            return out;
        }
        let mut index = 0;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(root, index, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            index += 1;
            let sub = String::from_utf16_lossy(&name[..len as usize]);
            if let Some(path) = reg_string(root, &sub, w!("ExecutablePath")) {
                let tip = reg_string(root, &sub, w!("InitialTooltip")).unwrap_or_default();
                out.push((resolve_placeholder(&path, known_folder), tip));
            }
        }
        let _ = RegCloseKey(root);
    }
    out
}

/// Имена файлов запущенных процессов (нижний регистр), без открытия самих процессов.
pub fn running_exe_names() -> HashSet<String> {
    let mut names = HashSet::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return names };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            names.insert(String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase());
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    names
}

/// Приложения для приглушённых плиток: запущены, значок в родном трее, в хосте его нет.
pub fn fallback_apps(hosted: &HashSet<String>) -> Vec<FallbackApp> {
    select_fallbacks(&registry_entries(), &running_exe_names(), hosted)
}

struct Search {
    name: String,
    found: Option<HWND>,
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || GetWindowTextLengthW(hwnd) == 0 {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if super::host::exe_of_pid(pid).is_some_and(|p| file_name_lower(&p) == search.name) {
            search.found = Some(hwnd);
            return false.into();
        }
    }
    true.into()
}

/// Клик по приглушённой плитке: окно приложения выходит вперёд, если окна нет, приложение запускается
/// (однопоточные приложения сами покажут свой запущенный экземпляр).
pub fn activate_app(exe_path: &str) {
    let mut search = Search { name: file_name_lower(exe_path), found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
        if let Some(hwnd) = search.found {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        } else {
            let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
            ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn entries(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect()
    }

    #[test]
    fn running_unhosted_apps_become_fallbacks_once() {
        let e = entries(&[
            (r"C:\Apps\Steam.exe", "Steam"),
            (r"D:\Other\steam.exe", "дубль"),
            (r"C:\Apps\gone.exe", "не запущен"),
            (r"C:\Apps\Hosted.exe", "уже в хосте"),
            (r"C:\Windows\explorer.exe", "система"),
        ]);
        let got = select_fallbacks(&e, &set(&["steam.exe", "hosted.exe", "explorer.exe"]), &set(&["hosted.exe"]));
        assert_eq!(got, vec![FallbackApp { exe_path: r"C:\Apps\Steam.exe".into(), tip: "Steam".into() }]);
    }

    #[test]
    fn known_folder_placeholder_is_replaced() {
        let lookup = |g: &str| (g == "7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E").then(|| r"C:\Program Files (x86)".to_string());
        assert_eq!(
            resolve_placeholder(r"{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}\Steam\steam.exe", lookup),
            r"C:\Program Files (x86)\Steam\steam.exe"
        );
        assert_eq!(resolve_placeholder(r"C:\a\b.exe", lookup), r"C:\a\b.exe");
        assert_eq!(resolve_placeholder(r"{unknown}\x.exe", lookup), r"{unknown}\x.exe");
    }

    #[test]
    fn real_registry_and_process_list_are_readable() {
        let running = running_exe_names();
        assert!(running.contains("explorer.exe"), "explorer must be in the process list");
        let entries = registry_entries();
        // На ПК без трей-приложений записей может не быть, но чтение не должно падать.
        assert!(entries.iter().all(|(p, _)| !p.is_empty()));
    }
}
