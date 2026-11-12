//! Индекс для Spotlight: приложения (классические и UWP) из `shell:AppsFolder`, страницы настроек Windows,
//! частоты запусков. Строится в фоновом потоке и лежит в памяти.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::Config;
use crate::search::{normalize, score, swap_layout};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Setting,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub title: String,
    /// `normalize(title)`: считается один раз при построении.
    pub norm: String,
    /// Дополнительные слова поиска (английские названия, синонимы), уже нормализованные.
    pub keywords: Vec<String>,
    /// Что открывает `ShellExecute`: `shell:AppsFolder\...` или `ms-settings:...`.
    pub target: String,
    pub kind: Kind,
    /// Имя значка из набора интерфейса (`gear`, `speaker`, ...); у приложений пусто, там настоящий значок.
    pub glyph: &'static str,
}

impl Entry {
    fn new(title: &str, keywords: &str, target: &str, kind: Kind, glyph: &'static str) -> Entry {
        Entry {
            title: title.to_string(),
            norm: normalize(title),
            keywords: keywords.split(',').map(normalize).filter(|k| !k.is_empty()).collect(),
            target: target.to_string(),
            kind,
            glyph,
        }
    }
}

/// Страницы настроек Windows: название по-русски, слова поиска на двух языках, адрес `ms-settings:`.
pub fn settings_entries() -> Vec<Entry> {
    const PAGES: &[(&str, &str, &str, &str)] = &[
        ("Дисплей", "display,screen,resolution,brightness,экран,разрешение,яркость,монитор", "display", "monitor"),
        ("Звук", "sound,audio,volume,speakers,microphone,громкость,динамики,микрофон", "sound", "speaker"),
        ("Уведомления", "notifications,оповещения", "notifications", "gear"),
        ("Фокусировка", "focus,do not disturb,не беспокоить", "quiethours", "gear"),
        ("Питание и батарея", "power,battery,sleep,питание,батарея,спящий режим,сон", "powersleep", "battery-full"),
        ("Экономия заряда", "battery saver,экономия батареи", "batterysaver", "battery-full"),
        ("Память", "storage,disk,хранилище,диск,очистка", "storagesense", "gear"),
        ("Многозадачность", "multitasking,snap,windows,привязка", "multitasking", "gear"),
        ("Буфер обмена", "clipboard", "clipboard", "gear"),
        ("О системе", "about,pc info,device name,версия windows,имя компьютера", "about", "gear"),
        ("Bluetooth и устройства", "bluetooth,devices,устройства,блютуз", "bluetooth", "gear"),
        ("Принтеры и сканеры", "printers,scanners,принтер,сканер", "printers", "gear"),
        ("Мышь", "mouse,pointer,курсор", "mousetouchpad", "gear"),
        ("Сенсорная панель", "touchpad,тачпад", "devices-touchpad", "gear"),
        ("Ввод", "typing,keyboard,клавиатура,автозамена", "typing", "gear"),
        ("Сеть и интернет", "network,internet,сеть,интернет", "network", "wifi"),
        ("Wi-Fi", "wifi,wireless,беспроводная сеть,вайфай", "network-wifi", "wifi"),
        ("Ethernet", "ethernet,lan,проводная сеть", "network-ethernet", "wifi"),
        ("VPN", "vpn,впн", "network-vpn", "wifi"),
        ("Режим в самолёте", "airplane mode,flight,авиарежим", "network-airplanemode", "wifi"),
        ("Прокси", "proxy,прокси-сервер", "network-proxy", "wifi"),
        ("Мобильный хот-спот", "hotspot,tethering,раздача интернета", "network-mobilehotspot", "wifi"),
        ("Персонализация", "personalization,персонализировать", "personalization", "gear"),
        ("Фон", "background,wallpaper,обои,рабочий стол", "personalization-background", "monitor"),
        ("Цвета", "colors,dark mode,accent,тёмная тема,режим,акцент", "colors", "gear"),
        ("Темы", "themes,тема оформления", "themes", "gear"),
        ("Экран блокировки", "lock screen", "lockscreen", "gear"),
        ("Панель задач", "taskbar,панель задач", "taskbar", "gear"),
        ("Ночной свет", "night light,ночной режим", "nightlight", "monitor"),
        ("Автозагрузка", "startup,startup apps,автозапуск", "startupapps", "gear"),
        ("Установленные приложения", "apps,features,uninstall,programs,приложения,программы,удалить", "appsfeatures", "gear"),
        ("Приложения по умолчанию", "default apps,defaults,по умолчанию", "defaultapps", "gear"),
        ("Учётные записи", "accounts,your info,account,аккаунт,пользователь", "yourinfo", "gear"),
        ("Параметры входа", "sign-in,password,pin,windows hello,пароль,пин-код", "signinoptions", "gear"),
        ("Электронная почта и учётные записи", "email,mail,почта", "emailandaccounts", "gear"),
        ("Дата и время", "date,time,clock,timezone,часовой пояс,часы", "dateandtime", "gear"),
        ("Язык и регион", "language,region,keyboard layout,язык,регион,раскладка", "regionlanguage", "gear"),
        ("Специальные возможности", "accessibility,ease of access,доступность", "easeofaccess", "gear"),
        ("Конфиденциальность и защита", "privacy,security,приватность", "privacy", "gear"),
        ("Центр обновления Windows", "windows update,updates,обновления,обновить", "windowsupdate", "gear"),
        ("Безопасность Windows", "windows security,defender,antivirus,защитник,антивирус", "windowsdefender", "gear"),
        ("Восстановление", "recovery,reset,reinstall,сброс,восстановить", "recovery", "gear"),
        ("Для разработчиков", "developers,developer mode,разработчик", "developers", "gear"),
        ("Игры", "gaming,game bar,игровой режим", "gaming", "gear"),
        ("Камеры", "cameras,webcam,камера,веб-камера", "camera", "gear"),
        ("Параметры Windows", "settings,параметры,настройки", "", "gear"),
    ];
    PAGES
        .iter()
        .map(|&(title, keywords, page, glyph)| Entry::new(title, keywords, &format!("ms-settings:{page}"), Kind::Setting, glyph))
        .collect()
}

/// Пропускает ли индекс приложение с таким названием: ярлыки удаления, справки и сайтов в списке не нужны.
pub fn filter_app(name: &str) -> bool {
    let n = normalize(name);
    if n.is_empty() {
        return false;
    }
    const NOISE: &[&str] = &[
        "удалить",
        "удаление",
        "деинсталл",
        "uninstall",
        "справка",
        "руководство",
        "документация",
        "documentation",
        "readme",
        "release notes",
        "веб-сайт",
        "website",
        "лицензионное",
        "license agreement",
    ];
    !NOISE.iter().any(|bad| n.contains(bad))
}

/// Годится ли адрес записи `shell:AppsFolder`: документы и сайты в Пуске лежат рядом с приложениями, а нам нужны
/// только программы (адреса вроде `steam://rungameid/570` для игр остаются).
pub fn filter_target(id: &str) -> bool {
    let lower = id.to_lowercase();
    const DOCS: &[&str] = &[".html", ".htm", ".chm", ".pdf", ".txt", ".rtf", ".md", ".url", ".doc", ".docx", ".xls"];
    let web = lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("file://");
    !web && !DOCS.iter().any(|ext| lower.ends_with(ext))
}

/// Файл, на который указывает адрес записи `shell:AppsFolder`: обычный путь или `{GUID известной папки}\остаток`.
/// Для пакетов (`Имя_издатель!App`), `steam://` и прочего, что не путь, `None`.
fn path_of_id(id: &str) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    use windows::core::GUID;

    let id = id.trim();
    if let Some(rest) = id.strip_prefix('{') {
        let (guid, tail) = rest.split_once('}')?;
        if !tail.starts_with('\\') {
            return None;
        }
        let guid = GUID::try_from(guid).ok()?;
        let folder = unsafe {
            let p = SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s?
        };
        return Some(PathBuf::from(format!("{}{}", folder.trim_end_matches('\\'), tail)));
    }
    let drive = id.len() > 3 && id.as_bytes()[1] == b':' && id.as_bytes()[2] == b'\\' && id.as_bytes()[0].is_ascii_alphabetic();
    (drive || id.starts_with("\\\\")).then(|| PathBuf::from(id))
}

/// Приложение удалено, а ярлык в Пуске остался: адрес указывает на файл, которого нет. Адреса, которые не
/// путь (пакеты, ссылки), и недоступные пути не считаются устаревшими: лучше показать лишнее, чем потерять живое.
pub fn is_stale(id: &str) -> bool {
    path_of_id(id).is_some_and(|p| matches!(p.try_exists(), Ok(false)))
}

/// Слова поиска из адреса записи: у русского «Калькулятор» адрес `Microsoft.WindowsCalculator_..!App`, и по
/// `calc` он находится. Для exe берётся имя файла без расширения. Служебные части адреса отбрасываются.
pub fn target_keywords(id: &str) -> Vec<String> {
    const GENERIC: &[&str] = &["com", "org", "net", "app", "desktop", "microsoft", "windows", "autogenerated", "exe", "settings"];
    let id = id.trim();
    if id.starts_with('{') && !id.contains('\\') || id.contains("://") {
        return Vec::new();
    }
    let stem = if id.contains(['\\', '/']) {
        let file = id.rsplit(['\\', '/']).next().unwrap_or(id);
        file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file).to_string()
    } else {
        id.split(['!', '_']).next().unwrap_or(id).to_string()
    };
    let mut out: Vec<String> = Vec::new();
    for part in stem.split(['.', ' ']) {
        let k = normalize(part);
        if k.chars().count() < 3 || GENERIC.contains(&k.as_str()) || k.starts_with('{') || out.contains(&k) {
            continue;
        }
        out.push(k);
    }
    out
}

/// Приложения из `shell:AppsFolder`: то же, что показывает Пуск в «Все приложения». `None`, если оболочка не ответила.
fn apps_from_shell() -> Option<Vec<Entry>> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IBindCtx};
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, KF_FLAG_DEFAULT, SHGetKnownFolderItem,
        SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None).ok()?;
        let items: IEnumShellItems = folder.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems).ok()?;
        let take = |item: &IShellItem, kind| -> Option<String> {
            let p = item.GetDisplayName(kind).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        };
        let mut out = Vec::new();
        loop {
            let mut batch: [Option<IShellItem>; 1] = [None];
            let mut fetched = 0u32;
            if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = batch[0].take() else { break };
            let (Some(name), Some(id)) = (take(&item, SIGDN_NORMALDISPLAY), take(&item, SIGDN_PARENTRELATIVEPARSING)) else {
                continue;
            };
            if filter_app(&name) && filter_target(&id) && !is_stale(&id) {
                out.push(Entry::new(&name, &target_keywords(&id).join(","), &format!("shell:AppsFolder\\{id}"), Kind::App, ""));
            }
        }
        Some(out)
    }
}

/// Всё, что ищет Spotlight: приложения и страницы настроек. Долго (сотни ярлыков): вызывать не в потоке окна.
pub fn build_index() -> Vec<Entry> {
    let cfg = Config::load();
    let mut apps = apps_from_shell().unwrap_or_default();
    apps.retain(|e| !cfg.is_app_hidden(&e.target));
    apps.sort_by(|a, b| a.norm.cmp(&b.norm));
    apps.dedup_by(|a, b| a.norm == b.norm);
    apps.extend(settings_entries());
    apps
}

/// Сколько раз запускали каждую цель. Лежит в `usage.txt` рядом с конфигом, строки вида `число<TAB>цель`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    counts: HashMap<String, u32>,
}

/// Больше записей в файле не хранится: остаются самые частые.
const USAGE_KEEP: usize = 300;

impl Usage {
    pub fn parse(text: &str) -> Usage {
        let counts = text
            .lines()
            .filter_map(|l| {
                let (n, target) = l.split_once('\t')?;
                Some((target.to_string(), n.trim().parse().ok()?))
            })
            .collect();
        Usage { counts }
    }

    pub fn serialize(&self) -> String {
        let mut rows: Vec<(&String, &u32)> = self.counts.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        rows.iter().take(USAGE_KEEP).map(|(t, n)| format!("{n}\t{t}\n")).collect()
    }

    fn path() -> Option<PathBuf> {
        Some(Config::path()?.with_file_name("usage.txt"))
    }

    pub fn load() -> Usage {
        Usage::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Usage::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Usage::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.serialize())
    }

    pub fn bump(&mut self, target: &str) {
        *self.counts.entry(target.to_string()).or_insert(0) += 1;
    }

    pub fn count(&self, target: &str) -> u32 {
        self.counts.get(target).copied().unwrap_or(0)
    }
}

/// Лучшие совпадения с запросом: `(номер записи, оценка)`, сначала самые подходящие.
/// Запрос ищется как набран и как если бы раскладка была другой (штраф 100); частые запуски поднимают запись.
pub fn rank(query: &str, entries: &[Entry], usage: &Usage, limit: usize) -> Vec<(usize, i32)> {
    let q = normalize(query);
    if q.is_empty() {
        return Vec::new();
    }
    let swapped = swap_layout(&q);
    let variants: Vec<(&str, i32)> =
        if swapped == q { vec![(q.as_str(), 0)] } else { vec![(q.as_str(), 0), (swapped.as_str(), -100)] };
    let mut hits: Vec<(usize, i32)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let best = variants
                .iter()
                .filter_map(|&(v, penalty)| {
                    let by_title = score(v, &e.norm);
                    let by_keyword = e.keywords.iter().filter_map(|k| score(v, k)).max().map(|s| s - 150);
                    Some(by_title.into_iter().chain(by_keyword).max()? + penalty)
                })
                .max()?;
            Some((i, best + usage.count(&e.target).min(20) as i32 * 8))
        })
        .collect();
    hits.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(entries[a.0].norm.chars().count().cmp(&entries[b.0].norm.chars().count()))
            .then(entries[a.0].norm.cmp(&entries[b.0].norm))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(title: &str) -> Entry {
        Entry::new(title, "", &format!("shell:AppsFolder\\{title}"), Kind::App, "")
    }

    #[test]
    fn settings_list_is_complete_and_addressable() {
        let list = settings_entries();
        assert!(list.len() >= 40);
        assert!(list.iter().all(|e| e.target.starts_with("ms-settings:") && e.kind == Kind::Setting));
        assert!(list.iter().any(|e| e.target == "ms-settings:sound"));
        let mut titles: Vec<_> = list.iter().map(|e| e.norm.clone()).collect();
        titles.sort();
        let n = titles.len();
        titles.dedup();
        assert_eq!(titles.len(), n, "duplicate setting titles");
    }

    #[test]
    fn filter_drops_uninstallers_and_help_but_keeps_apps() {
        for bad in ["Удалить Steam", "Uninstall Foo", "Справка по Word", "Readme", "Веб-сайт Notepad++", ""] {
            assert!(!filter_app(bad), "{bad:?}");
        }
        for good in ["Калькулятор", "Visual Studio Code", "Параметры", "Telegram"] {
            assert!(filter_app(good), "{good:?}");
        }
    }

    #[test]
    fn only_paths_to_missing_files_are_stale() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(is_stale(r"C:\definitely\missing\app.exe"));
        assert!(!is_stale(&format!(r"{root}\explorer.exe")));
        // {System32}: известная папка раскрывается, файл проверяется.
        assert!(is_stale(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\no-such-tool-xyz.exe"));
        assert!(!is_stale(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe"));
        for not_a_path in ["Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", "steam://rungameid/570", "Chrome", r"{unknown}\x.exe", "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}"] {
            assert!(!is_stale(not_a_path), "{not_a_path:?} must stay listed");
        }
    }

    #[test]
    fn filter_target_keeps_programs_and_drops_documents_and_sites() {
        for bad in [
            r"{6D809377-6AF0-444B-8957-A3773F02200E}\7-Zip\7-zip.chm",
            r"{6D809377-6AF0-444B-8957-A3773F02200E}\Foo\docs\instructions.html",
            "https://gitforwindows.org/faq",
            r"file://C:\Program Files\PostgreSQL\18/doc/installation-notes.html",
        ] {
            assert!(!filter_target(bad), "{bad:?}");
        }
        for good in [
            r"{6D809377-6AF0-444B-8957-A3773F02200E}\7-Zip\7zFM.exe",
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            "steam://rungameid/570",
            "Chrome",
        ] {
            assert!(filter_target(good), "{good:?}");
        }
    }

    #[test]
    fn target_keywords_give_english_names_to_localized_apps() {
        assert_eq!(target_keywords("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), vec!["windowscalculator"]);
        assert_eq!(target_keywords(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Git\cmd\git-gui.exe"), vec!["git-gui"]);
        assert_eq!(target_keywords("com.squirrel.Discord.Discord"), vec!["squirrel", "discord"]);
        assert_eq!(target_keywords(r"C:\Games\Cyberpunk 2077\bin\x64\Cyberpunk2077.exe"), vec!["cyberpunk2077"]);
        assert!(target_keywords("steam://rungameid/570").is_empty());
        assert!(target_keywords("{6D809377-6AF0-444B-8957-A3773F02200E}").is_empty());
        assert!(target_keywords("Microsoft.AutoGenerated.{96690F80-0B7F}").is_empty());
    }

    #[test]
    fn english_query_finds_a_localized_app_by_its_package_name() {
        let calc = Entry::new(
            "Калькулятор",
            &target_keywords("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App").join(","),
            "shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            Kind::App,
            "",
        );
        let entries = vec![calc];
        assert_eq!(rank("calc", &entries, &Usage::default(), 3).len(), 1);
        assert_eq!(rank("кальк", &entries, &Usage::default(), 3).len(), 1);
    }

    #[test]
    fn usage_round_trips_and_keeps_the_most_used() {
        let mut u = Usage::default();
        for _ in 0..3 {
            u.bump("a");
        }
        u.bump("b");
        let text = u.serialize();
        assert_eq!(text, "3\ta\n1\tb\n");
        assert_eq!(Usage::parse(&text), u);
        assert_eq!(u.count("a"), 3);
        assert_eq!(u.count("zzz"), 0);
        assert_eq!(Usage::parse("garbage\n\n5 no tab\nx\ty\n").count("y"), 0);
    }

    #[test]
    fn rank_finds_by_title_and_keyword() {
        let entries = vec![app("Калькулятор"), app("Блокнот")];
        let mut all = entries.clone();
        all.extend(settings_entries());
        let usage = Usage::default();
        let top = |q: &str| all[rank(q, &all, &usage, 5)[0].0].title.clone();
        assert_eq!(top("калькул"), "Калькулятор");
        assert_eq!(top("bluetooth"), "Bluetooth и устройства");
        assert_eq!(top("громкость"), "Звук"); // слово поиска, которого нет в названии
    }

    #[test]
    fn rank_reads_a_query_typed_in_the_wrong_layout() {
        let all = settings_entries();
        let usage = Usage::default();
        // «ыщгтв» набрано в русской раскладке вместо «sound»: ключевое слово «sound» у страницы «Звук».
        let hit = &all[rank("ыщгтв", &all, &usage, 3)[0].0];
        assert_eq!(hit.title, "Звук");
        // Наоборот: «pder» вместо «звук».
        let hit = &all[rank("pder", &all, &usage, 3)[0].0];
        assert_eq!(hit.title, "Звук");
    }

    #[test]
    fn frequent_launches_lift_an_entry_over_an_equal_one() {
        let entries = vec![app("Notepad A"), app("Notepad B")];
        let mut usage = Usage::default();
        let first = |u: &Usage| rank("notepad", &entries, u, 2)[0].0;
        assert_eq!(first(&usage), 0); // при равенстве по алфавиту
        for _ in 0..3 {
            usage.bump(&entries[1].target);
        }
        assert_eq!(first(&usage), 1);
    }

    #[test]
    fn rank_limits_and_ignores_empty_queries() {
        let all = settings_entries();
        let usage = Usage::default();
        assert!(rank("   ", &all, &usage, 5).is_empty());
        assert!(rank("н", &all, &usage, 3).len() <= 3);
    }

    /// Показывает, что нашлось на этом ПК и сколько заняло: `cargo test -p macshell --lib list_real_index -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn list_real_index() {
        let t = std::time::Instant::now();
        let idx = build_index();
        println!("{} entries in {:?}", idx.len(), t.elapsed());
        for e in idx.iter().filter(|e| e.kind == Kind::App) {
            println!("{:<40} {}", e.title, e.target);
        }
    }

    #[test]
    fn real_start_menu_has_apps() {
        // Живая оболочка: на рабочем столе владельца приложений заведомо больше десятка.
        let idx = build_index();
        let apps = idx.iter().filter(|e| e.kind == Kind::App).count();
        assert!(apps > 10, "only {apps} apps found");
        assert!(idx.iter().any(|e| e.kind == Kind::Setting));
    }
}
