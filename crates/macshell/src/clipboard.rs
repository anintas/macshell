//! История буфера обмена для вкладки «Буфер» в Spotlight. Следит окно панели (`WM_CLIPBOARDUPDATE`), хранятся
//! только тексты, только в памяти и не дольше `MAX_AGE`. Пароли не попадают: менеджеры паролей помечают копию
//! форматом `ExcludeClipboardContentFromMonitorProcessing`, а их окна ещё и отсеиваются по имени программы.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, GetClipboardOwner, IsClipboardFormatAvailable, RegisterClipboardFormatW,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::w;

pub const MAX_ITEMS: usize = 100;
/// Длиннее не запоминаем: это обычно не фраза, а файл или лог.
pub const MAX_LEN: usize = 20_000;
pub const MAX_AGE: Duration = Duration::from_secs(8 * 3600);
pub const WM_CLIPBOARDUPDATE: u32 = 0x031D;

/// Программы, чей буфер не запоминается (менеджеры паролей).
const EXCLUDED_APPS: &[&str] = &["keepass.exe", "keepassxc.exe", "1password.exe", "bitwarden.exe"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipItem {
    pub text: String,
    /// Время копирования, секунды от начала эпохи Unix.
    pub at: u64,
}

/// Список копий, новые впереди.
#[derive(Default)]
pub struct History {
    items: Vec<ClipItem>,
}

impl History {
    /// Запоминает текст. Повтор уже известного поднимается наверх. `false`, если текст не годится (пустой,
    /// из одних пробелов, слишком длинный).
    pub fn push(&mut self, text: &str, now: u64) -> bool {
        if text.trim().is_empty() || text.chars().count() > MAX_LEN {
            return false;
        }
        self.items.retain(|i| i.text != text);
        self.items.insert(0, ClipItem { text: text.to_string(), at: now });
        self.items.truncate(MAX_ITEMS);
        true
    }

    /// Выбрасывает копии старше `MAX_AGE`.
    pub fn expire(&mut self, now: u64) {
        self.items.retain(|i| now.saturating_sub(i.at) <= MAX_AGE.as_secs());
    }

    pub fn items(&self) -> &[ClipItem] {
        &self.items
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

static HISTORY: Mutex<History> = Mutex::new(History { items: Vec::new() });

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Просит систему слать окну `hwnd` сообщение при каждом изменении буфера.
pub fn listen(hwnd: isize) {
    unsafe {
        let _ = AddClipboardFormatListener(HWND(hwnd as *mut _));
    }
}

/// Копия помечена как не предназначенная для истории или сделана менеджером паролей.
fn excluded() -> bool {
    unsafe {
        let marker = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        if marker != 0 && IsClipboardFormatAvailable(marker).is_ok() {
            return true;
        }
        let Ok(owner) = GetClipboardOwner() else { return false };
        let mut pid = 0u32;
        GetWindowThreadProcessId(owner, Some(&mut pid));
        crate::tray::host::exe_of_pid(pid)
            .and_then(|p| p.rsplit(['\\', '/']).next().map(str::to_lowercase))
            .is_some_and(|name| EXCLUDED_APPS.contains(&name.as_str()))
    }
}

/// Буфер изменился: запоминает его текст, если можно.
pub fn capture() {
    if excluded() {
        return;
    }
    let Some(text) = crate::spotlight::clipboard_text() else { return };
    if let Ok(mut h) = HISTORY.lock() {
        let now = now_secs();
        h.expire(now);
        h.push(&text, now);
    }
}

/// Копии от новых к старым; устаревшие уже выброшены.
pub fn snapshot() -> Vec<ClipItem> {
    let Ok(mut h) = HISTORY.lock() else { return Vec::new() };
    h.expire(now_secs());
    h.items().to_vec()
}

pub fn clear() {
    if let Ok(mut h) = HISTORY.lock() {
        h.clear();
    }
}

/// «сейчас», «5 мин», «3 ч»: сколько прошло с копирования.
pub fn age_label(secs: u64) -> String {
    match secs {
        0..=44 => "сейчас".to_string(),
        45..=3599 => format!("{} мин", secs.div_ceil(60)),
        _ => format!("{} ч", secs / 3600),
    }
}

/// Первая непустая строка текста, не длиннее `max` символов (с многоточием).
pub fn preview(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > max {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_and_repeat_moves_to_the_top() {
        let mut h = History::default();
        assert!(h.push("a", 1) && h.push("b", 2) && h.push("a", 3));
        let texts: Vec<&str> = h.items().iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, vec!["a", "b"]);
        assert_eq!(h.items()[0].at, 3);
    }

    #[test]
    fn blank_and_oversized_texts_are_refused() {
        let mut h = History::default();
        assert!(!h.push("", 1) && !h.push("  \n\t", 1));
        assert!(!h.push(&"x".repeat(MAX_LEN + 1), 1));
        assert!(h.push(&"x".repeat(MAX_LEN), 1));
        assert_eq!(h.items().len(), 1);
    }

    #[test]
    fn history_is_capped_and_expires() {
        let mut h = History::default();
        for i in 0..MAX_ITEMS + 20 {
            h.push(&format!("item {i}"), 100);
        }
        assert_eq!(h.items().len(), MAX_ITEMS);
        assert_eq!(h.items()[0].text, format!("item {}", MAX_ITEMS + 19));
        h.push("fresh", 100 + MAX_AGE.as_secs() + 10);
        h.expire(100 + MAX_AGE.as_secs() + 10);
        assert_eq!(h.items().len(), 1, "everything older than the retention is dropped");
    }

    #[test]
    fn age_labels_use_minutes_then_hours() {
        assert_eq!(age_label(5), "сейчас");
        assert_eq!(age_label(60), "1 мин");
        assert_eq!(age_label(125), "3 мин");
        assert_eq!(age_label(3600), "1 ч");
        assert_eq!(age_label(7300), "2 ч");
    }

    #[test]
    fn preview_takes_the_first_nonblank_line_and_trims_long_ones() {
        assert_eq!(preview("\n\n  hello  \nworld", 40), "hello");
        assert_eq!(preview(&"я".repeat(50), 10).chars().count(), 10);
        assert!(preview(&"я".repeat(50), 10).ends_with('…'));
        assert_eq!(preview("   ", 10), "");
    }
}
