//! Индикаторы приватности, как оранжевая и зелёная точки в строке меню macOS: какая программа сейчас
//! пользуется микрофоном или камерой. Windows ведёт это в `CapabilityAccessManager`: у каждой программы
//! есть время последнего начала и конца использования, и пока конца нет позже начала, устройство занято.
//! Изменения реестра ловит отдельный поток без опроса, поэтому в покое процессор не тратится.

use std::sync::Once;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_VALUE_TYPE, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
};
use windows::core::{PCWSTR, PWSTR};

const STORE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Microphone,
    Camera,
}

impl Device {
    fn key(self) -> &'static str {
        match self {
            Device::Microphone => "microphone",
            Device::Camera => "webcam",
        }
    }
}

/// Занято ли устройство: начало использования позже конца (или конца ещё нет).
pub fn in_use(start: u64, stop: u64) -> bool {
    start != 0 && start > stop
}

/// Имя программы из ключа реестра: `C:#Program Files#Zoom#zoom.exe` даёт `zoom`; пакет `Microsoft.WindowsCamera_8wek...`
/// даёт `Microsoft.WindowsCamera`.
pub fn app_name(key: &str) -> String {
    let leaf = key.rsplit(['#', '\\', '/']).next().unwrap_or(key);
    let leaf = leaf.split('_').next().unwrap_or(leaf);
    let lower = leaf.to_ascii_lowercase();
    lower.strip_suffix(".exe").map(|_| leaf[..leaf.len() - 4].to_string()).unwrap_or_else(|| leaf.to_string())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open(parent: HKEY, path: &str, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
    let p = wide(path);
    let mut key = HKEY::default();
    (unsafe { RegOpenKeyExW(parent, PCWSTR(p.as_ptr()), None, access, &mut key) } == ERROR_SUCCESS).then_some(key)
}

fn subkeys(key: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    loop {
        let mut name = vec![0u16; 512];
        let mut len = name.len() as u32;
        let status = unsafe { RegEnumKeyExW(key, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        if status != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..len as usize]));
        index += 1;
    }
    out
}

fn qword(key: HKEY, name: &str) -> u64 {
    let n = wide(name);
    let mut value = 0u64;
    let mut size = 8u32;
    let mut kind = REG_VALUE_TYPE::default();
    let status = unsafe {
        RegQueryValueExW(key, PCWSTR(n.as_ptr()), None, Some(&mut kind), Some(&mut value as *mut u64 as *mut u8), Some(&mut size))
    };
    if status == ERROR_SUCCESS && size == 8 { value } else { 0 }
}

/// Программы, которые сейчас пользуются устройством (по именам, без повторов).
pub fn users(device: Device) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let Some(root) = open(HKEY_CURRENT_USER, &format!("{STORE}\\{}", device.key()), KEY_READ) else { return found };
    let mut check = |parent: HKEY, name: &str| {
        if let Some(app) = open(parent, name, KEY_READ) {
            if in_use(qword(app, "LastUsedTimeStart"), qword(app, "LastUsedTimeStop")) {
                let shown = app_name(name);
                if !found.contains(&shown) {
                    found.push(shown);
                }
            }
            unsafe {
                let _ = RegCloseKey(app);
            }
        }
    };
    for name in subkeys(root) {
        if name == "NonPackaged" {
            if let Some(np) = open(root, &name, KEY_READ) {
                for app in subkeys(np) {
                    check(np, &app);
                }
                unsafe {
                    let _ = RegCloseKey(np);
                }
            }
        } else {
            check(root, &name);
        }
    }
    unsafe {
        let _ = RegCloseKey(root);
    }
    found
}

/// Занято ли устройство хоть одной программой.
pub fn active(device: Device) -> bool {
    !users(device).is_empty()
}

static WATCHING: Once = Once::new();

/// Запускает поток, который вызывает `wake` при любом изменении под `ConsentStore`. Вызывается один раз за жизнь процесса.
pub fn watch(wake: fn()) {
    WATCHING.call_once(|| crate::regwatch::spawn("privacy-watch", STORE, wake));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_is_busy_while_start_is_after_stop() {
        assert!(in_use(200, 100));
        assert!(in_use(200, 0), "no stop yet");
        assert!(!in_use(100, 200));
        assert!(!in_use(0, 0), "never used");
        assert!(!in_use(100, 100));
    }

    #[test]
    fn names_come_from_exe_paths_and_package_ids() {
        assert_eq!(app_name("C:#Program Files#Zoom#bin#Zoom.exe"), "Zoom");
        assert_eq!(app_name("Microsoft.WindowsCamera_8wekyb3d8bbwe"), "Microsoft.WindowsCamera");
        assert_eq!(app_name("plain"), "plain");
    }

    #[test]
    fn real_registry_reads_without_panic() {
        let _ = users(Device::Microphone);
        let _ = users(Device::Camera);
    }
}
