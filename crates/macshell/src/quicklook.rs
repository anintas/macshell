//! Предпросмотр файла в Spotlight (Quick Look из macOS): название, тип, размер, возраст и либо начало
//! текстового файла, либо миниатюра, которую даёт оболочка Windows (картинки, PDF, видео). Миниатюру грузит
//! `icons::load_icon`; здесь только подпись и текст, они не требуют окна и проверяются тестами.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::files::FileEntry;

/// Сколько строк текста показывать и сколько знаков в строке.
pub const TEXT_LINES: usize = 12;
pub const LINE_CHARS: usize = 64;
/// Из файла читается не больше столько байт: хватает на первые строки, большие файлы не грузятся.
const HEAD_BYTES: usize = 4096;

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "log", "csv", "json", "toml", "yaml", "yml", "xml", "html", "htm", "css", "js", "ts", "rs", "py", "ini", "cfg",
    "conf", "ps1", "bat", "cmd", "sh", "slint", "c", "h", "cpp", "java", "go", "sql", "tsx", "jsx", "kt", "swift", "lua", "rb",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    pub title: String,
    /// «PNG · 2,4 МБ · изменён 3 ч назад».
    pub meta: String,
    /// Начало текстового файла; пусто у остальных.
    pub text: String,
}

/// 512 Б, 38 КБ, 2,4 МБ, 1,1 ГБ.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["Б", "КБ", "МБ", "ГБ"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes} Б");
    }
    let shown = if value >= 100.0 { format!("{value:.0}") } else { format!("{value:.1}").replace('.', ",") };
    format!("{} {}", shown.trim_end_matches(",0"), UNITS[unit])
}

/// Расширение считается текстовым.
pub fn is_text(path: &str) -> bool {
    Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| TEXT_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// Первые строки текста: пустые строки в начале пропускаются, длинные обрезаются многоточием, табуляция становится
/// пробелами, управляющие знаки отбрасываются. Файл с нулевыми байтами считается двоичным и даёт пустую строку.
pub fn head_lines(bytes: &[u8], lines: usize, width: usize) -> String {
    if bytes.contains(&0) {
        return String::new();
    }
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().skip_while(|l| l.trim().is_empty()).take(lines) {
        let clean: String = line.replace('\t', "    ").chars().filter(|c| !c.is_control()).collect();
        if clean.chars().count() > width {
            let cut: String = clean.chars().take(width.saturating_sub(1)).collect();
            out.push(format!("{cut}…"));
        } else {
            out.push(clean);
        }
    }
    out.join("\n")
}

/// Описание файла для панели предпросмотра. Без доступа к файлу остаются название и тип.
pub fn describe(entry: &FileEntry, kind: &str, now_secs: u64) -> Preview {
    let mut parts = vec![kind.to_string()];
    let meta = std::fs::metadata(&entry.path).ok();
    if let Some(m) = &meta {
        if !entry.dir {
            parts.push(human_size(m.len()));
        }
    }
    if entry.mtime > 0 {
        parts.push(format!("изменён {}", age(now_secs.saturating_sub(entry.mtime))));
    }
    let text = if !entry.dir && is_text(&entry.path) {
        let mut buf = vec![0u8; HEAD_BYTES];
        let read = File::open(&entry.path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
        head_lines(&buf[..read], TEXT_LINES, LINE_CHARS)
    } else {
        String::new()
    };
    Preview { title: entry.name.clone(), meta: parts.join(" · "), text }
}

/// «только что», «5 мин назад», «3 ч назад», «2 дн. назад», «4 мес. назад».
pub fn age(secs: u64) -> String {
    match secs {
        0..=59 => "только что".to_string(),
        60..=3599 => format!("{} мин назад", secs / 60),
        3600..=86_399 => format!("{} ч назад", secs / 3600),
        86_400..=2_591_999 => format!("{} дн. назад", secs / 86_400),
        _ => format!("{} мес. назад", secs / 2_592_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(512), "512 Б");
        assert_eq!(human_size(38 * 1024), "38 КБ");
        assert_eq!(human_size(2_516_582), "2,4 МБ");
        assert_eq!(human_size(1024 * 1024 * 1024 + 100_000_000), "1,1 ГБ");
        assert_eq!(human_size(150 * 1024 * 1024), "150 МБ");
    }

    #[test]
    fn only_known_extensions_are_text() {
        assert!(is_text("C:\\a\\Notes.TXT"));
        assert!(is_text("main.rs"));
        assert!(!is_text("photo.png"));
        assert!(!is_text("noext"));
    }

    #[test]
    fn head_lines_skips_blank_start_trims_and_cuts() {
        let long = "x".repeat(100);
        let text = format!("\n\n  first\tline\nsecond\n{long}\nfourth");
        let got = head_lines(text.as_bytes(), 3, 10);
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "  first  …", "tab becomes spaces, long line cut with an ellipsis");
        assert_eq!(lines[1], "second");
        assert_eq!(lines[2], "xxxxxxxxx…");
    }

    #[test]
    fn binary_and_bom_inputs_are_handled() {
        assert_eq!(head_lines(&[b'a', 0, b'b'], 5, 20), "", "zero byte means binary");
        assert_eq!(head_lines("\u{feff}hello".as_bytes(), 5, 20), "hello");
        assert_eq!(head_lines(b"", 5, 20), "");
    }

    #[test]
    fn age_buckets() {
        assert_eq!(age(10), "только что");
        assert_eq!(age(300), "5 мин назад");
        assert_eq!(age(7300), "2 ч назад");
        assert_eq!(age(3 * 86_400), "3 дн. назад");
        assert_eq!(age(95 * 86_400), "3 мес. назад");
    }

    #[test]
    fn describe_reads_a_real_text_file() {
        let path = std::env::temp_dir().join(format!("macshell-quicklook-{}.txt", std::process::id()));
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let entry = FileEntry {
            path: path.to_string_lossy().into_owned(),
            name: "q.txt".into(),
            norm: "q.txt".into(),
            dir: false,
            mtime: 1_000,
        };
        let p = describe(&entry, "TXT", 1_000 + 7200);
        let _ = std::fs::remove_file(&path);
        assert_eq!(p.title, "q.txt");
        assert_eq!(p.meta, "TXT · 11 Б · изменён 2 ч назад");
        assert_eq!(p.text, "alpha\nbeta");
    }

    #[test]
    fn missing_file_keeps_title_and_kind() {
        let entry = FileEntry { path: "C:\\no\\such\\file.txt".into(), name: "file.txt".into(), norm: "file.txt".into(), dir: false, mtime: 0 };
        let p = describe(&entry, "TXT", 0);
        assert_eq!((p.title.as_str(), p.meta.as_str(), p.text.as_str()), ("file.txt", "TXT", ""));
    }
}
