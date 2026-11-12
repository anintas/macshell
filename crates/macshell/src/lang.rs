//! Раскладка клавиатуры для верхней полосы: двухбуквенный код языка окна в фокусе и переключение на следующую.
//! Смену раскладки приносит оболочка: `RegisterShellHookWindow` присылает окну `HSHELL_LANGUAGE` с новой раскладкой.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Globalization::{GetLocaleInfoW, LOCALE_SISO639LANGNAME};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, RegisterShellHookWindow, RegisterWindowMessageW,
    WM_INPUTLANGCHANGEREQUEST,
};
use windows::core::w;

/// Значение `wParam` у сообщения оболочки, когда в окне в фокусе сменилась раскладка (`lParam` это новая раскладка).
pub const HSHELL_LANGUAGE: usize = 0x19;

/// Подписывает окно на сообщения оболочки и возвращает их номер. Так же подписывается Dock на окна приложений.
pub fn register_shell_hook(hwnd: isize) -> u32 {
    unsafe {
        let _ = RegisterShellHookWindow(HWND(hwnd as *mut _));
        RegisterWindowMessageW(w!("SHELLHOOK"))
    }
}

/// Код языка из идентификатора раскладки: «RU», «EN». Частые языки из таблицы, остальные спрашивает система.
pub fn code_from_langid(langid: u16) -> String {
    let known = match langid & 0x3FF {
        0x09 => "EN",
        0x19 => "RU",
        0x22 => "UK",
        0x23 => "BE",
        0x3F => "KK",
        0x07 => "DE",
        0x0C => "FR",
        0x0A => "ES",
        0x10 => "IT",
        0x16 => "PT",
        0x15 => "PL",
        0x05 => "CS",
        0x1F => "TR",
        0x04 => "ZH",
        0x11 => "JA",
        0x12 => "KO",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    let mut buf = [0u16; 16];
    let n = unsafe { GetLocaleInfoW(u32::from(langid), LOCALE_SISO639LANGNAME, Some(&mut buf)) } as usize;
    if n > 1 {
        String::from_utf16_lossy(&buf[..n - 1]).to_uppercase()
    } else {
        "??".to_string()
    }
}

/// Идентификатор языка раскладки (`HKL`): младшие 16 бит.
pub fn langid_of_hkl(hkl: isize) -> u16 {
    (hkl as usize & 0xFFFF) as u16
}

/// Раскладка окна в фокусе.
pub fn current_langid() -> u16 {
    unsafe {
        let thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        langid_of_hkl(GetKeyboardLayout(thread).0 as isize)
    }
}

pub fn current() -> String {
    code_from_langid(current_langid())
}

/// Просит окно в фокусе перейти на следующую раскладку (как Alt+Shift).
pub fn switch_next() {
    unsafe {
        let _ = PostMessageW(Some(GetForegroundWindow()), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(1)); // HKL_NEXT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_layouts_have_short_codes() {
        assert_eq!(code_from_langid(0x0409), "EN", "английский (США)");
        assert_eq!(code_from_langid(0x0809), "EN", "английский (Великобритания)");
        assert_eq!(code_from_langid(0x0419), "RU");
        assert_eq!(code_from_langid(0x0422), "UK");
    }

    #[test]
    fn unknown_primary_language_asks_the_system() {
        // Шведский: в таблице нет, система отвечает «sv».
        assert_eq!(code_from_langid(0x041D), "SV");
    }

    #[test]
    fn hkl_low_word_is_the_language() {
        // Старшие 16 бит HKL это устройство, не язык.
        assert_eq!(langid_of_hkl(0x0419_0419), 0x0419);
        assert_eq!(langid_of_hkl(0x04090409_u32 as isize), 0x0409);
    }
}
