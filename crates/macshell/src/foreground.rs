//! Название активного приложения для верхней полосы. Обновляется событием `EVENT_SYSTEM_FOREGROUND`.

use std::sync::Once;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, PostMessageW,
    WINEVENT_OUTOFCONTEXT,
};
use windows::core::PWSTR;

const DESKTOP: &str = "Рабочий стол";
const MAX_CHARS: usize = 28;

/// Имя для полосы: имя exe с заглавной буквы; для UWP (`ApplicationFrameHost`) заголовок окна;
/// длинное обрезается до 28 символов с `…`.
pub fn app_name(exe_path: Option<&str>, title: &str) -> String {
    let stem = exe_path
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .map(|file| file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file));
    let name = match stem {
        Some(s) if s.eq_ignore_ascii_case("ApplicationFrameHost") => title.to_string(),
        Some(s) if s.eq_ignore_ascii_case("explorer") => "Проводник".to_string(),
        Some(s) if !s.is_empty() => {
            let mut chars = s.chars();
            chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        }
        _ => title.to_string(),
    };
    if name.is_empty() {
        return DESKTOP.to_string();
    }
    if name.chars().count() > MAX_CHARS {
        let cut: String = name.chars().take(MAX_CHARS - 1).collect();
        return format!("{cut}…");
    }
    name
}

fn clip(name: String) -> String {
    if name.chars().count() > MAX_CHARS {
        let cut: String = name.chars().take(MAX_CHARS - 1).collect();
        return format!("{cut}…");
    }
    name
}

/// Общее название продукта вместо имени приложения: так подписаны системные exe.
fn is_generic_product(product: &str) -> bool {
    let p = product.to_lowercase();
    p.contains("operating system") || p.contains("операционная система") || p.contains("microsoft® windows")
}

/// Название из ресурсов exe: имя продукта, если оно конкретное, иначе описание файла, иначе имя файла.
/// Так сделаны и подписи в диспетчере задач. Длинное обрезается до 28 символов.
pub fn pick_app_name(description: Option<&str>, product: Option<&str>, stem: &str) -> String {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    let name = clean(product)
        .filter(|p| !is_generic_product(p))
        .or(clean(description))
        .map(str::to_string)
        .unwrap_or_else(|| {
            let mut chars = stem.chars();
            chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        });
    clip(name)
}

thread_local! {
    static NAMES: std::cell::RefCell<std::collections::HashMap<String, Option<String>>> = Default::default();
}

/// Название приложения из ресурсов версии exe (кэшируется по пути). `None`, если ресурсов нет.
pub fn resource_name(path: &str) -> Option<String> {
    if let Some(hit) = NAMES.with(|c| c.borrow().get(path).cloned()) {
        return hit;
    }
    let name = read_version_strings(path).map(|(d, p)| {
        let stem = path.rsplit(['\\', '/']).next().unwrap_or(path);
        let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem);
        pick_app_name(d.as_deref(), p.as_deref(), stem)
    });
    NAMES.with(|c| c.borrow_mut().insert(path.to_string(), name.clone()));
    name
}

/// `(FileDescription, ProductName)` из ресурсов версии на первом языке файла.
fn read_version_strings(path: &str) -> Option<(Option<String>, Option<String>)> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wide.as_ptr()), None, size, data.as_mut_ptr() as *mut _).ok()?;

        let query = |sub: &str| -> Option<(*const u16, u32)> {
            let sub: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut len = 0u32;
            VerQueryValueW(data.as_ptr() as *const _, PCWSTR(sub.as_ptr()), &mut ptr, &mut len)
                .as_bool()
                .then_some((ptr as *const u16, len))
        };
        // Первая пара «язык, кодовая страница»; без неё берём английский (0409, 04B0).
        let (lang, page) = match query("\\VarFileInfo\\Translation") {
            Some((p, len)) if len >= 4 => (*p, *p.add(1)),
            _ => (0x0409, 0x04B0),
        };
        let get = |key: &str| -> Option<String> {
            let (p, len) = query(&format!("\\StringFileInfo\\{lang:04x}{page:04x}\\{key}"))?;
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(p, len as usize));
            let text = text.trim_end_matches('\0').trim().to_string();
            (!text.is_empty()).then_some(text)
        };
        let (description, product) = (get("FileDescription"), get("ProductName"));
        (description.is_some() || product.is_some()).then_some((description, product))
    }
}

static NOTIFY_HWND: AtomicIsize = AtomicIsize::new(0);
static NOTIFY_MSG: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn on_foreground(_h: HWINEVENTHOOK, _event: u32, _hwnd: HWND, _obj: i32, _child: i32, _thread: u32, _time: u32) {
    let target = NOTIFY_HWND.load(Ordering::Relaxed);
    if target != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(target as *mut _)), NOTIFY_MSG.load(Ordering::Relaxed), WPARAM(0), LPARAM(0));
        }
    }
}

/// Шлёт окну `hwnd` сообщение `msg` при каждой смене активного окна. Нужен цикл сообщений в этом потоке.
pub fn watch(hwnd: isize, msg: u32) {
    NOTIFY_HWND.store(hwnd, Ordering::Relaxed);
    NOTIFY_MSG.store(msg, Ordering::Relaxed);
    // Хук ставится один раз: при пересборке полосы меняется только адресат сообщения.
    HOOK.call_once(|| unsafe {
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);
    });
}

static HOOK: Once = Once::new();

fn wide_to_string(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())])
}

/// Впереди окно самой оболочки (Spotlight): название приложения на панели остаётся прежним, как в macOS.
pub fn foreground_is_own() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut class = [0u16; 32];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        wide_to_string(&class[..n]) == "macshell_layered"
    }
}

/// Название приложения в активном окне.
pub fn current() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return DESKTOP.to_string();
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        if matches!(wide_to_string(&class[..n]).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return DESKTOP.to_string();
        }
        let mut title = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut title) as usize;
        let title = wide_to_string(&title[..n]);

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let mut path = None;
        if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = [0u16; 520];
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok() {
                path = Some(wide_to_string(&buf[..len as usize]));
            }
            let _ = CloseHandle(process);
        }
        // UWP-окна живут внутри ApplicationFrameHost: у них название берётся из заголовка окна.
        let is_frame_host = path.as_deref().is_some_and(|p| p.to_lowercase().ends_with("applicationframehost.exe"));
        match path.as_deref().filter(|_| !is_frame_host).and_then(resource_name) {
            Some(name) => name,
            None => app_name(path.as_deref(), &title),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_desktop() {
        assert_eq!(app_name(None, ""), "Рабочий стол");
    }

    #[test]
    fn frame_host_uses_title() {
        assert_eq!(app_name(Some(r"C:\Windows\System32\ApplicationFrameHost.exe"), "Калькулятор"), "Калькулятор");
    }

    #[test]
    fn strips_exe_and_capitalizes() {
        assert_eq!(app_name(Some(r"C:\x\chrome.exe"), "Новая вкладка"), "Chrome");
    }

    #[test]
    fn explorer_is_localized() {
        assert_eq!(app_name(Some(r"C:\Windows\explorer.exe"), "Загрузки"), "Проводник");
    }

    #[test]
    fn truncates_long_names() {
        let name = app_name(None, &"а".repeat(40));
        assert_eq!(name.chars().count(), 28);
        assert!(name.ends_with('…'));
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn product_name_beats_a_clumsy_description() {
        assert_eq!(pick_app_name(Some("Steam Client Bootstrapper"), Some("Steam"), "steam"), "Steam");
    }

    #[test]
    fn generic_product_falls_back_to_description() {
        let generic = Some("Microsoft® Windows® Operating System");
        assert_eq!(pick_app_name(Some("Notepad"), generic, "notepad"), "Notepad");
        assert_eq!(pick_app_name(Some("Windows Explorer"), Some("Операционная система Microsoft® Windows®"), "explorer"), "Windows Explorer");
    }

    #[test]
    fn no_resources_uses_capitalized_stem() {
        assert_eq!(pick_app_name(None, None, "chrome"), "Chrome");
        assert_eq!(pick_app_name(Some("  "), Some(""), "winword"), "Winword");
    }

    #[test]
    fn long_names_are_clipped() {
        let long = "а".repeat(50);
        let n = pick_app_name(None, Some(&long), "x");
        assert_eq!(n.chars().count(), 28);
        assert!(n.ends_with('…'));
    }

    #[test]
    fn notepad_name_comes_from_file_resources() {
        let path = format!("{}\\System32\\notepad.exe", std::env::var("SystemRoot").unwrap());
        let name = resource_name(&path).expect("notepad has version resources");
        assert!(!name.is_empty() && !name.to_lowercase().contains("operating system"), "{name}");
    }

    #[test]
    fn missing_file_has_no_resource_name() {
        assert!(resource_name(r"C:\no\such\file.exe").is_none());
    }
}
