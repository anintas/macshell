//! Тёмная и светлая тема Windows: чтение и переключение (значения `AppsUseLightTheme` и `SystemUsesLightTheme`
//! в `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`), как «Тёмный режим» в Control Center.

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
use windows::core::{PCWSTR, w};

const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const VALUES: [PCWSTR; 2] = [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")];

fn read(value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, KEY, value, RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut _), Some(&mut size))
            .ok()
            .ok()?;
    }
    Some(data)
}

/// Приложения Windows сейчас тёмные. Без значения в реестре тема светлая.
pub fn is_dark() -> bool {
    read(VALUES[0]) == Some(0)
}

/// Включает тёмную или светлую тему для приложений и системы и сообщает об этом окнам (`ImmersiveColorSet`).
pub fn set_dark(dark: bool) {
    let light: u32 = if dark { 0 } else { 1 };
    unsafe {
        for value in VALUES {
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, KEY, value, REG_DWORD.0, Some(&light as *const u32 as *const _), 4);
        }
        let mut result = 0usize;
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("ImmersiveColorSet").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            200,
            Some(&mut result),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_is_readable_without_panicking() {
        // Значение зависит от машины; важно только, что чтение возвращается.
        let _ = is_dark();
    }
}
