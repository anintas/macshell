//! Наблюдение за веткой реестра без опроса: поток спит на `RegNotifyChangeKeyValue` и вызывает `wake`, когда под
//! веткой что-то записали. Так следят за занятостью микрофона и камеры и за режимом «Не беспокоить».

use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::PCWSTR;

/// Запускает поток для ветки `path` в `HKEY_CURRENT_USER` (вместе с вложенными). Если ветки нет, поток тихо выходит.
pub fn spawn(name: &str, path: &'static str, wake: fn()) {
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut root = HKEY::default();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()), None, KEY_NOTIFY, &mut root) } != ERROR_SUCCESS {
            return;
        }
        let Ok(event) = (unsafe { CreateEventW(None, false, false, PCWSTR::null()) }) else { return };
        loop {
            let armed = unsafe { RegNotifyChangeKeyValue(root, true, REG_NOTIFY_CHANGE_LAST_SET, Some(event), true) };
            if armed != ERROR_SUCCESS {
                break;
            }
            unsafe { WaitForSingleObject(event, INFINITE) };
            wake();
        }
        unsafe {
            let _ = CloseHandle(event);
            let _ = RegCloseKey(root);
        }
    });
}
