//! Вкладка «Файлы» в Spotlight: индекс имён файлов и папок в личных папках пользователя (Рабочий стол,
//! Документы, Загрузки, Изображения, Видео, Музыка). Строится один раз в фоновом потоке, потом лежит в памяти
//! (названия и время изменения, без содержимого). Папки с зависимостями и скрытые файлы пропускаются.

use std::collections::VecDeque;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::search::{match_score, normalize};

pub const MAX_ENTRIES: usize = 25_000;
const MAX_DEPTH: usize = 5;
/// Индекс не строится дольше этого: на медленном диске лучше неполный список, чем долгое ожидание.
const BUDGET: Duration = Duration::from_secs(4);
const SKIP_DIRS: &[&str] = &["node_modules", "target", "__pycache__", "venv", "site-packages", "$recycle.bin"];
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    /// `normalize(name)`: считается один раз при построении.
    pub norm: String,
    pub dir: bool,
    /// Время изменения, секунды от начала эпохи Unix.
    pub mtime: u64,
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// Папки, которые индексируются.
pub fn roots() -> Vec<PathBuf> {
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos,
    };
    [FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures, FOLDERID_Videos, FOLDERID_Music]
        .iter()
        .filter_map(known_folder)
        .filter(|p| p.is_dir())
        .collect()
}

/// Идёт вширь от каждого корня; останавливается на `max_entries` записях или по истечении `budget`.
pub fn build_from(roots: &[PathBuf], max_entries: usize, budget: Duration) -> Vec<FileEntry> {
    let started = Instant::now();
    let mut out = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = roots.iter().cloned().map(|r| (r, 0)).collect();
    while let Some((dir, depth)) = queue.pop_front() {
        if out.len() >= max_entries || started.elapsed() > budget {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                continue;
            }
            let is_dir = meta.is_dir();
            let path = entry.path();
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            if is_dir && depth < MAX_DEPTH && !SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
                queue.push_back((path.clone(), depth + 1));
            }
            out.push(FileEntry { path: path.to_string_lossy().into_owned(), norm: normalize(&name), name, dir: is_dir, mtime });
            if out.len() >= max_entries {
                break;
            }
        }
    }
    out
}

/// Индекс личных папок. Долго: вызывать не в потоке окна.
pub fn build() -> Vec<FileEntry> {
    build_from(&roots(), MAX_ENTRIES, BUDGET)
}

/// Недавно изменённые файлы (папки не считаются): то, что показывается при пустом запросе.
pub fn recent(files: &[FileEntry], limit: usize) -> Vec<&FileEntry> {
    let mut v: Vec<&FileEntry> = files.iter().filter(|f| !f.dir).collect();
    v.sort_by(|a, b| b.mtime.cmp(&a.mtime).then(a.norm.cmp(&b.norm)));
    v.truncate(limit);
    v
}

/// Лучшие совпадения по имени; при равной оценке новее выше, при близком совпадении папки чуть выше файлов.
pub fn search<'a>(files: &'a [FileEntry], query: &str, limit: usize) -> Vec<&'a FileEntry> {
    let mut hits: Vec<(i32, &FileEntry)> =
        files.iter().filter_map(|f| Some((match_score(query, &f.norm, &[])? + if f.dir { 5 } else { 0 }, f))).collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.mtime.cmp(&a.1.mtime)).then(a.1.norm.cmp(&b.1.norm)));
    hits.truncate(limit);
    hits.into_iter().map(|(_, f)| f).collect()
}

/// Подпись справа: «Папка» или расширение («PDF»), иначе «Файл».
pub fn kind_label(entry: &FileEntry) -> String {
    if entry.dir {
        return "Папка".to_string();
    }
    match Path::new(&entry.name).extension().and_then(|e| e.to_str()) {
        Some(ext) if !ext.is_empty() && ext.len() <= 5 => ext.to_uppercase(),
        _ => "Файл".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, dir: bool, mtime: u64) -> FileEntry {
        FileEntry { path: format!(r"C:\x\{name}"), name: name.into(), norm: normalize(name), dir, mtime }
    }

    #[test]
    fn walk_finds_nested_files_and_skips_hidden_and_dependency_dirs() {
        let root = std::env::temp_dir().join(format!("macshell-files-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("docs").join("deep")).unwrap();
        std::fs::create_dir_all(root.join("node_modules").join("pkg")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("docs").join("report.pdf"), b"x").unwrap();
        std::fs::write(root.join("docs").join("deep").join("notes.txt"), b"x").unwrap();
        std::fs::write(root.join("node_modules").join("pkg").join("index.js"), b"x").unwrap();
        std::fs::write(root.join(".git").join("config"), b"x").unwrap();
        std::fs::write(root.join(".hidden"), b"x").unwrap();

        let found = build_from(&[root.clone()], 1000, Duration::from_secs(5));
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        let _ = std::fs::remove_dir_all(&root);
        assert!(names.contains(&"report.pdf") && names.contains(&"notes.txt") && names.contains(&"docs"));
        assert!(names.contains(&"node_modules"), "the folder itself is listed, its content is not");
        assert!(!names.contains(&"index.js") && !names.contains(&".git") && !names.contains(&".hidden"), "{names:?}");
    }

    #[test]
    fn walk_stops_at_the_entry_cap() {
        let root = std::env::temp_dir().join(format!("macshell-files-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..20 {
            std::fs::write(root.join(format!("f{i}.txt")), b"x").unwrap();
        }
        let found = build_from(&[root.clone()], 5, Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(found.len(), 5);
    }

    #[test]
    fn recent_lists_files_by_modification_time_without_folders() {
        let files = vec![entry("old.txt", false, 10), entry("dir", true, 99), entry("new.txt", false, 50)];
        let names: Vec<&str> = recent(&files, 5).iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["new.txt", "old.txt"]);
        assert_eq!(recent(&files, 1).len(), 1);
    }

    #[test]
    fn search_matches_names_and_ignores_the_layout() {
        let files = vec![entry("report final.docx", false, 5), entry("report.pdf", false, 9), entry("photo.png", false, 99)];
        assert_eq!(search(&files, "report", 5).len(), 2);
        assert!(search(&files, "zzzzq", 5).is_empty());
        // «report» набрано в русской раскладке.
        assert_eq!(search(&files, "кузщке", 5).len(), 2);
    }

    #[test]
    fn kind_label_uses_the_extension() {
        assert_eq!(kind_label(&entry("a.pdf", false, 0)), "PDF");
        assert_eq!(kind_label(&entry("Makefile", false, 0)), "Файл");
        assert_eq!(kind_label(&entry("docs", true, 0)), "Папка");
    }
}
