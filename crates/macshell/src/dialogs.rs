//! Системный диалог «Открыть файл». Блокирует поток, пока открыт: вызывать после закрытия меню.

use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::core::PCWSTR;

/// Строка фильтра для диалога: пары «название\0шаблоны\0», в конце двойной нуль.
fn filter_string(name: &str, patterns: &str) -> Vec<u16> {
    let mut s: Vec<u16> = name.encode_utf16().collect();
    s.push(0);
    s.extend(patterns.encode_utf16());
    s.extend([0, 0]);
    s
}

/// Выбор файла. `patterns` через точку с запятой: `*.exe;*.lnk`. `None`, если пользователь отменил.
pub fn pick_file(title: &str, filter_name: &str, patterns: &str) -> Option<String> {
    let filter = filter_string(filter_name, patterns);
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_is_double_nul_terminated_pairs() {
        let f = filter_string("Приложения", "*.exe;*.lnk");
        let text = String::from_utf16_lossy(&f);
        assert_eq!(text, "Приложения\0*.exe;*.lnk\0\0");
    }
}
