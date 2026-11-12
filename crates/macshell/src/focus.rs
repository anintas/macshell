//! «Не беспокоить» (Фокусировка в macOS): в Windows 11 это значение `NOC_GLOBAL_SETTING_TOASTS_ENABLED` в
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Notifications\Settings`; ноль значит, что уведомления не показываются.
//! Автоматические правила (по расписанию, при показе экрана) Windows не отдаёт, поэтому здесь только ручной
//! выключатель, который тот же, что в центре уведомлений.

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW};
use windows::core::{PCWSTR, w};

pub const KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings";
const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings");
const VALUE: PCWSTR = w!("NOC_GLOBAL_SETTING_TOASTS_ENABLED");

/// Режим включён, когда показ уведомлений явно выключен. Нет значения значит, что уведомления идут как обычно.
pub fn on_from(toasts_enabled: Option<u32>) -> bool {
    toasts_enabled == Some(0)
}

pub fn is_on() -> bool {
    let mut data = 0u32;
    let mut size = 4u32;
    let read = unsafe { RegGetValueW(HKEY_CURRENT_USER, KEY, VALUE, RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut _), Some(&mut size)) };
    on_from(read.ok().ok().map(|_| data))
}

/// Включает или выключает «Не беспокоить». Центр уведомлений следит за значением и подхватывает его сам.
pub fn set(on: bool) {
    let enabled: u32 = if on { 0 } else { 1 };
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, KEY, VALUE, REG_DWORD.0, Some(&enabled as *const u32 as *const _), 4);
    }
}

/// Сообщает `wake`, когда режим сменили (здесь или в системе).
pub fn watch(wake: fn()) {
    crate::regwatch::spawn("focus-watch", KEY_PATH, wake);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_zero_means_do_not_disturb() {
        assert!(on_from(Some(0)));
        assert!(!on_from(Some(1)));
        assert!(!on_from(None), "no value: notifications work");
    }

    #[test]
    fn state_is_readable() {
        let _ = is_on();
    }
}
