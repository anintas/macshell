//! Маленький конфиг `%APPDATA%\macshell\config.toml`: свой логотип и закреплённые на панели приложения.
//! Понимает только строки `ключ = "значение"`; остальные строки и комментарии сохраняет как есть.

use std::path::PathBuf;

use crate::clock::ClockOptions;
use crate::tray::fallback::file_name_lower;

/// Как показывать Dock (`dock = "off"`, `"autohide"` или `"fixed"`; без ключа он умный).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DockMode {
    /// Места не занимает: виден над рабочим столом, прячется под окнами, которые до него достают.
    #[default]
    Smart,
    /// Занимает полосу внизу экрана, окна приложений не заходят под него.
    Fixed,
    Autohide,
    /// Dock не показывается, родная панель задач остаётся на месте.
    Off,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Путь к картинке или значку для логотипа; `None` значит стандартный.
    pub logo: Option<String>,
    /// Закреплённые на панели приложения: пути к `.exe` или `.lnk`, в порядке слева направо.
    pub pins: Vec<String>,
    /// `tray = "off"`: не занимать трей, оставить значки родному трею Проводника.
    pub tray_off: bool,
    /// `tray_icons = "mono"`: значки трея одноцветные, под стиль панели.
    pub tray_mono: bool,
    /// Значки трея, вынесенные на панель: пути exe приложений (`tray_pin = "..."`).
    pub tray_pins: Vec<String>,
    /// `spotlight = "off"`: не перехватывать Win, Пуск работает как обычно.
    pub spotlight_off: bool,
    /// `win_start = "keep"`: одиночный Win остаётся Пуском (по умолчанию он открывает Dock и сетку приложений).
    pub win_start_keep: bool,
    /// `switcher = "off"`: Alt+Tab остаётся системным (по умолчанию его заменяет переключатель приложений).
    pub switcher_off: bool,
    /// `stage = "on"`: Stage Manager, на сцене одно приложение, остальные в полоске у левого края.
    pub stage_on: bool,
    pub dock: DockMode,
    /// Закреплённые в Dock приложения (`dock_pin`): пути exe или `shell:AppsFolder\AUMID`.
    pub dock_pins: Vec<String>,
    /// Начальный набор закреплённых уже положен (`dock_init`): очищенный Dock снова не наполняется.
    pub dock_seeded: bool,
    /// `bar = "solid"`: панель всегда сплошная. По умолчанию она прозрачная, пока виден рабочий стол, и
    /// сплошная, когда окно приложения закрывает его.
    pub bar_solid: bool,
    /// `bar_color = "#RRGGBB"`: цвет сплошной панели; чёрный, как чёлка Island без своего цвета.
    pub bar_color: [u8; 3],
    /// `accent = "#RRGGBB"`: цвет системы (выбранные плитки, кружки, шкалы); без ключа синий.
    pub accent: Option<[u8; 3]>,
    /// Плотность тонировки меню, в процентах; отсутствие оставляет прежние 40%.
    pub glass_density: Option<u8>,
    /// Горячие углы (`corner_tl`, `corner_tr`, `corner_bl`, `corner_br`): что делает курсор, ушедший в угол экрана.
    pub corners: [String; 4],
    /// Быстрые клавиши (`quick_key = "lk=lock"`): короткий запрос и то, что он запускает, пары «запрос, цель».
    pub quick_keys: Vec<(String, String)>,
    /// `battery_percent = "on"`: рядом со значком батареи в панели показываются проценты.
    pub battery_percent: bool,
    /// `clock = "seconds,12h,nodate,noweekday"`: что показывают часы; пусто значит «Пн 2 нояб. 20:31».
    pub clock: ClockOptions,
    /// `bar_order = "battery,wifi,..."`: порядок значков состояния слева направо; пусто значит обычный.
    pub bar_order: String,
    /// `bar_hidden = "vpn,lang"`: значки состояния, которые не показываются.
    pub bar_hidden: String,
    /// `bar_autohide = "on"`: панель прячется и выезжает, когда курсор у верхнего края экрана.
    pub bar_autohide: bool,
    /// Виджеты на рабочем столе (`widget = "clock,40,64"`): вид и положение в логических пикселях.
    pub widgets: Vec<String>,
    /// `weather = "55.75,37.62,Москва"`: место для виджета погоды; без него место определяется по IP.
    pub weather: String,
    /// Значки трея, убранные из окна значков (`tray_hide`): пути exe, сравниваются по имени файла.
    pub tray_hidden: Vec<String>,
    /// Приложения, убранные из списка «Приложения» и Spotlight (`app_hide`): адреса `shell:AppsFolder\...`.
    pub apps_hidden: Vec<String>,
    /// Всё, что мы не разбираем (другие ключи, комментарии): возвращается при сохранении.
    rest: Vec<String>,
}

fn unquote(value: &str) -> Option<String> {
    let mut chars = value.trim().strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    None
}

/// `#RRGGBB` (или `RRGGBB`) в три байта.
pub fn parse_hex_color(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Config {
    pub fn parse(text: &str) -> Config {
        let mut cfg = Config::default();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed = trimmed.split_once('=').and_then(|(k, v)| Some((k.trim(), unquote(v)?)));
            match parsed {
                Some(("logo", v)) => cfg.logo = Some(v),
                Some(("pin", v)) => cfg.pins.push(v),
                Some(("tray", v)) => cfg.tray_off = v == "off",
                Some(("tray_icons", v)) => cfg.tray_mono = v == "mono",
                Some(("tray_pin", v)) => cfg.tray_pins.push(v),
                Some(("spotlight", v)) => cfg.spotlight_off = v == "off",
                Some(("win_start", v)) => cfg.win_start_keep = v == "keep",
                Some(("switcher", v)) => cfg.switcher_off = v == "off",
                Some(("stage", v)) => cfg.stage_on = v == "on",
                Some(("dock", v)) => {
                    cfg.dock = match v.as_str() {
                        "off" => DockMode::Off,
                        "autohide" => DockMode::Autohide,
                        "fixed" => DockMode::Fixed,
                        _ => DockMode::Smart,
                    }
                }
                Some(("battery_percent", v)) => cfg.battery_percent = v == "on",
                Some(("clock", v)) => cfg.clock = ClockOptions::parse(&v),
                Some(("bar_order", v)) => cfg.bar_order = v,
                Some(("bar_hidden", v)) => cfg.bar_hidden = v,
                Some(("bar_autohide", v)) => cfg.bar_autohide = v == "on",
                Some(("widget", v)) => cfg.widgets.push(v),
                Some(("weather", v)) => cfg.weather = v,
                Some(("bar", v)) => cfg.bar_solid = v == "solid",
                Some(("accent", v)) => cfg.accent = parse_hex_color(&v),
                Some(("glass_density", v)) => cfg.glass_density = v.parse::<u16>().ok().map(|n| n.clamp(10, 85) as u8),
                Some(("corner_tl", v)) => cfg.corners[0] = v,
                Some(("corner_tr", v)) => cfg.corners[1] = v,
                Some(("corner_bl", v)) => cfg.corners[2] = v,
                Some(("corner_br", v)) => cfg.corners[3] = v,
                Some(("bar_color", v)) => {
                    if let Some(rgb) = parse_hex_color(&v) {
                        cfg.bar_color = rgb;
                    }
                }
                Some(("quick_key", v)) => {
                    if let Some((trigger, target)) = v.split_once('=') {
                        let (trigger, target) = (trigger.trim(), target.trim());
                        if !trigger.is_empty() && !target.is_empty() {
                            cfg.quick_keys.push((trigger.to_string(), target.to_string()));
                        }
                    }
                }
                Some(("tray_hide", v)) => cfg.tray_hidden.push(v),
                Some(("app_hide", v)) => cfg.apps_hidden.push(v),
                Some(("dock_pin", v)) => cfg.dock_pins.push(v),
                Some(("dock_init", v)) => cfg.dock_seeded = v == "1",
                _ => cfg.rest.push(line.to_string()),
            }
        }
        cfg
    }

    pub fn serialize(&self) -> String {
        let mut lines = self.rest.clone();
        if let Some(logo) = &self.logo {
            lines.push(format!("logo = {}", quote(logo)));
        }
        lines.extend(self.pins.iter().map(|p| format!("pin = {}", quote(p))));
        if self.tray_off {
            lines.push("tray = \"off\"".to_string());
        }
        if self.tray_mono {
            lines.push("tray_icons = \"mono\"".to_string());
        }
        lines.extend(self.tray_pins.iter().map(|p| format!("tray_pin = {}", quote(p))));
        if self.stage_on {
            lines.push("stage = \"on\"".to_string());
        }
        if self.switcher_off {
            lines.push("switcher = \"off\"".to_string());
        }
        if self.win_start_keep {
            lines.push("win_start = \"keep\"".to_string());
        }
        if self.spotlight_off {
            lines.push("spotlight = \"off\"".to_string());
        }
        match self.dock {
            DockMode::Off => lines.push("dock = \"off\"".to_string()),
            DockMode::Autohide => lines.push("dock = \"autohide\"".to_string()),
            DockMode::Fixed => lines.push("dock = \"fixed\"".to_string()),
            DockMode::Smart => {}
        }
        if self.battery_percent {
            lines.push("battery_percent = \"on\"".to_string());
        }
        if !self.bar_order.is_empty() {
            lines.push(format!("bar_order = {}", quote(&self.bar_order)));
        }
        if !self.bar_hidden.is_empty() {
            lines.push(format!("bar_hidden = {}", quote(&self.bar_hidden)));
        }
        lines.extend(self.widgets.iter().map(|w| format!("widget = {}", quote(w))));
        if !self.weather.is_empty() {
            lines.push(format!("weather = {}", quote(&self.weather)));
        }
        if self.bar_autohide {
            lines.push("bar_autohide = \"on\"".to_string());
        }
        if self.clock != ClockOptions::default() {
            lines.push(format!("clock = {}", quote(&self.clock.serialize())));
        }
        if self.bar_solid {
            lines.push("bar = \"solid\"".to_string());
        }
        if let Some(a) = self.accent {
            lines.push(format!("accent = \"#{:02X}{:02X}{:02X}\"", a[0], a[1], a[2]));
        }
        if let Some(density) = self.glass_density {
            lines.push(format!("glass_density = \"{}\"", density.clamp(10, 85)));
        }
        for (key, value) in ["corner_tl", "corner_tr", "corner_bl", "corner_br"].iter().zip(&self.corners) {
            if !value.is_empty() {
                lines.push(format!("{key} = {}", quote(value)));
            }
        }
        if self.bar_color != [0, 0, 0] {
            lines.push(format!("bar_color = \"#{:02X}{:02X}{:02X}\"", self.bar_color[0], self.bar_color[1], self.bar_color[2]));
        }
        lines.extend(self.quick_keys.iter().map(|(k, t)| format!("quick_key = {}", quote(&format!("{k}={t}")))));
        lines.extend(self.tray_hidden.iter().map(|p| format!("tray_hide = {}", quote(p))));
        lines.extend(self.apps_hidden.iter().map(|p| format!("app_hide = {}", quote(p))));
        lines.extend(self.dock_pins.iter().map(|p| format!("dock_pin = {}", quote(p))));
        if self.dock_seeded {
            lines.push("dock_init = \"1\"".to_string());
        }
        if lines.is_empty() { String::new() } else { lines.join("\n") + "\n" }
    }

    /// Кладёт в конфиг подсказку про быстрые клавиши (комментарии), если её там ещё нет.
    pub fn add_quick_key_help(&mut self) {
        const MARK: &str = "# Быстрые клавиши Spotlight";
        if self.rest.iter().any(|l| l.starts_with(MARK)) {
            return;
        }
        self.rest.push(format!("{MARK}: запрос и цель через знак равенства."));
        self.rest.push("# Цель: lock, sleep, dark, bluetooth, mute, screenshot, taskmgr, trash, nightlight, terminal,".to_string());
        self.rest.push("# clearclip, logoff, restart, shutdown, либо путь, адрес сайта или shell:AppsFolder\\...".to_string());
        self.rest.push("# Пример (уберите решётку в начале): quick_key = \"gh=https://github.com\"".to_string());
    }

    /// Закрепляет или открепляет значок трея по пути exe (без учёта регистра). `true`, если теперь закреплён.
    pub fn toggle_tray_pin(&mut self, exe: &str) -> bool {
        match self.tray_pins.iter().position(|p| p.eq_ignore_ascii_case(exe)) {
            Some(i) => {
                self.tray_pins.remove(i);
                false
            }
            None => {
                self.tray_pins.push(exe.to_string());
                true
            }
        }
    }

    pub fn is_tray_pinned(&self, exe: &str) -> bool {
        self.tray_pins.iter().any(|p| p.eq_ignore_ascii_case(exe))
    }

    /// Прячет значок трея из окна значков или возвращает его (по имени файла exe, без учёта регистра и папки).
    /// `true`, если теперь спрятан. Спрятанный значок заодно снимается с панели.
    pub fn toggle_tray_hidden(&mut self, exe: &str) -> bool {
        let name = file_name_lower(exe);
        match self.tray_hidden.iter().position(|p| file_name_lower(p) == name) {
            Some(i) => {
                self.tray_hidden.remove(i);
                false
            }
            None => {
                self.tray_hidden.push(exe.to_string());
                self.tray_pins.retain(|p| file_name_lower(p) != name);
                true
            }
        }
    }

    pub fn is_tray_hidden(&self, exe: &str) -> bool {
        let name = file_name_lower(exe);
        self.tray_hidden.iter().any(|p| file_name_lower(p) == name)
    }

    /// Убирает приложение из списка «Приложения» и Spotlight.
    pub fn hide_app(&mut self, target: &str) {
        if !self.is_app_hidden(target) {
            self.apps_hidden.push(target.to_string());
        }
    }

    pub fn is_app_hidden(&self, target: &str) -> bool {
        self.apps_hidden.iter().any(|t| t.eq_ignore_ascii_case(target))
    }

    /// Закрепляет приложение в Dock или снимает закрепление (приложения сравниваются по `dock::key_of`).
    /// `true`, если теперь закреплено.
    pub fn toggle_dock_pin(&mut self, target: &str) -> bool {
        let key = crate::dock::key_of(target);
        match self.dock_pins.iter().position(|p| crate::dock::key_of(p) == key) {
            Some(i) => {
                self.dock_pins.remove(i);
                false
            }
            None => {
                self.dock_pins.push(target.to_string());
                true
            }
        }
    }

    pub fn remove_pin(&mut self, index: usize) {
        if index < self.pins.len() {
            self.pins.remove(index);
        }
    }

    /// `MACSHELL_CONFIG` (для тестов и отладки) или `%APPDATA%\macshell\config.toml`.
    pub fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("MACSHELL_CONFIG") {
            return Some(PathBuf::from(p));
        }
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("macshell").join("config.toml"))
    }

    /// Нет файла или он не читается: стандартные настройки.
    pub fn load() -> Config {
        Config::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Config::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Config::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.serialize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glass_density_is_optional_bounded_and_preserved() {
        let cfg = Config::parse("glass_density = \"67\"\n");
        assert_eq!(cfg.glass_density, Some(67));
        assert_eq!(Config::parse(&cfg.serialize()).glass_density, Some(67));
        assert_eq!(Config::parse("glass_density = \"200\"").glass_density, Some(85));
        assert_eq!(Config::parse("").glass_density, None);
    }

    #[test]
    fn parses_logo_and_pins() {
        let c = Config::parse("# комментарий\nlogo = \"C:\\\\img\\\\me.png\"\n\npin = \"C:\\\\a.exe\"\npin = \"D:\\\\b.lnk\"\n");
        assert_eq!(c.logo.as_deref(), Some(r"C:\img\me.png"));
        assert_eq!(c.pins, vec![r"C:\a.exe".to_string(), r"D:\b.lnk".to_string()]);
    }

    #[test]
    fn roundtrip_keeps_backslashes_quotes_and_unknown_lines() {
        let src = "language = \"ru\"\nlogo = \"C:\\\\x\\\\\\\"q\\\".png\"\npin = \"C:\\\\a.exe\"\n";
        let c = Config::parse(src);
        assert_eq!(c.logo.as_deref(), Some("C:\\x\\\"q\".png"));
        let again = Config::parse(&c.serialize());
        assert_eq!(again, c);
        assert!(c.serialize().contains("language = \"ru\""));
    }

    #[test]
    fn empty_config_is_default() {
        assert_eq!(Config::parse(""), Config::default());
        assert_eq!(Config::default().serialize(), "");
    }

    #[test]
    fn spotlight_switch_roundtrips() {
        assert!(!Config::parse("").spotlight_off);
        let c = Config::parse("spotlight = \"off\"\n");
        assert!(c.spotlight_off);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn remove_pin_by_index_ignores_out_of_range() {
        let mut c = Config { pins: vec!["a".into(), "b".into()], ..Config::default() };
        c.remove_pin(5);
        assert_eq!(c.pins.len(), 2);
        c.remove_pin(0);
        assert_eq!(c.pins, vec!["b".to_string()]);
    }
}

#[cfg(test)]
mod dock_tests {
    use super::*;

    #[test]
    fn dock_options_roundtrip() {
        let c = Config::parse("dock = \"autohide\"\ndock_pin = \"C:\\\\a\\\\x.exe\"\ndock_pin = \"shell:AppsFolder\\\\Chrome\"\ndock_init = \"1\"\n");
        assert_eq!(c.dock, DockMode::Autohide);
        assert_eq!(c.dock_pins, vec![r"C:\a\x.exe".to_string(), r"shell:AppsFolder\Chrome".to_string()]);
        assert!(c.dock_seeded);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn dock_defaults_to_smart_and_every_mode_roundtrips() {
        assert_eq!(Config::parse("").dock, DockMode::Smart);
        for (text, mode) in [("off", DockMode::Off), ("fixed", DockMode::Fixed), ("autohide", DockMode::Autohide)] {
            let c = Config::parse(&format!("dock = \"{text}\"\n"));
            assert_eq!(c.dock, mode);
            assert_eq!(Config::parse(&c.serialize()), c);
        }
        assert_eq!(Config::default().serialize(), "");
    }

    #[test]
    fn hidden_tray_icons_match_by_file_name_and_leave_the_bar() {
        let mut c = Config { tray_pins: vec![r"C:\Apps\Steam.exe".into()], ..Config::default() };
        assert!(c.toggle_tray_hidden(r"C:\Apps\Steam.exe"));
        assert!(c.is_tray_hidden(r"D:\other\STEAM.exe"));
        assert!(c.tray_pins.is_empty(), "hidden icon must leave the bar");
        assert_eq!(Config::parse(&c.serialize()), c);
        assert!(!c.toggle_tray_hidden(r"d:\x\steam.exe"));
        assert!(!c.is_tray_hidden(r"C:\Apps\Steam.exe"));
    }

    #[test]
    fn bar_is_adaptive_and_black_when_solid_by_default_and_options_roundtrip() {
        let d = Config::parse("");
        assert!(!d.bar_solid, "transparent over the desktop unless forced solid");
        assert_eq!(d.bar_color, [0, 0, 0]);
        assert_eq!(d.serialize(), "");
        let c = Config::parse("bar = \"solid\"
bar_color = \"#1a2B3c\"
");
        assert!(c.bar_solid);
        assert_eq!(c.bar_color, [0x1A, 0x2B, 0x3C]);
        assert_eq!(Config::parse(&c.serialize()), c);
        assert_eq!(Config::parse("bar_color = \"nonsense\"").bar_color, [0, 0, 0]);
    }

    #[test]
    fn quick_key_help_is_added_once_and_does_not_become_a_key() {
        let mut c = Config::parse("logo = \"x.png\"
");
        c.add_quick_key_help();
        c.add_quick_key_help();
        let text = c.serialize();
        assert_eq!(text.matches("# Быстрые клавиши Spotlight").count(), 1);
        let again = Config::parse(&text);
        assert!(again.quick_keys.is_empty(), "the example in the comment must stay a comment");
        assert_eq!(again.logo.as_deref(), Some("x.png"));
        assert_eq!(Config::parse(&again.serialize()), again);
    }

    #[test]
    fn stage_roundtrips_and_is_off_by_default() {
        assert!(!Config::parse("").stage_on && !Config::parse("").serialize().contains("stage"));
        let c = Config::parse("stage = \"on\"\n");
        assert!(c.stage_on);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn switcher_off_roundtrips_and_defaults_to_on() {
        assert!(!Config::parse("").switcher_off && !Config::parse("").serialize().contains("switcher"));
        let c = Config::parse("switcher = \"off\"\n");
        assert!(c.switcher_off);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn win_start_keep_roundtrips_and_defaults_to_the_grid() {
        assert!(!Config::parse("").win_start_keep && !Config::parse("").serialize().contains("win_start"));
        let c = Config::parse("win_start = \"keep\"\n");
        assert!(c.win_start_keep);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn accent_and_corners_roundtrip() {
        let c = Config::parse("accent = \"#a03cff\"\ncorner_tl = \"mission\"\ncorner_br = \"desktop\"\n");
        assert_eq!(c.accent, Some([0xA0, 0x3C, 0xFF]));
        assert_eq!(c.corners, ["mission".to_string(), String::new(), String::new(), "desktop".to_string()]);
        assert_eq!(Config::parse(&c.serialize()), c);
        let empty = Config::parse("").serialize();
        assert!(!empty.contains("accent") && !empty.contains("corner"));
        assert_eq!(Config::parse("accent = \"nonsense\"").accent, None);
    }

    #[test]
    fn widgets_are_kept_one_line_each() {
        let c = Config::parse("widget = \"clock,8,48\"\nwidget = \"system,8,208\"\n");
        assert_eq!(c.widgets, vec!["clock,8,48", "system,8,208"]);
        assert_eq!(Config::parse(&c.serialize()), c);
        assert!(!Config::parse("").serialize().contains("widget"));
        let w = Config::parse("weather = \"55.75,37.62,Москва\"\n");
        assert_eq!(w.weather, "55.75,37.62,Москва");
        assert_eq!(Config::parse(&w.serialize()), w);
    }

    #[test]
    fn bar_items_and_autohide_roundtrip_and_default_to_nothing_written() {
        let empty = Config::parse("").serialize();
        assert!(!empty.contains("bar_order") && !empty.contains("bar_hidden") && !empty.contains("bar_autohide"));
        let c = Config::parse("bar_order = \"wifi,battery\"\nbar_hidden = \"vpn\"\nbar_autohide = \"on\"\n");
        assert_eq!((c.bar_order.as_str(), c.bar_hidden.as_str(), c.bar_autohide), ("wifi,battery", "vpn", true));
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn clock_options_roundtrip_and_default_to_nothing_written() {
        assert_eq!(Config::parse("").clock, ClockOptions::default());
        assert!(!Config::parse("").serialize().contains("clock"));
        let c = Config::parse("clock = \"seconds,nodate\"\n");
        assert!(c.clock.seconds && c.clock.hide_date && !c.clock.hour12);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn battery_percent_is_off_by_default_and_roundtrips() {
        assert!(!Config::parse("").battery_percent);
        let c = Config::parse("battery_percent = \"on\"
");
        assert!(c.battery_percent);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn quick_keys_roundtrip_and_ignore_broken_lines() {
        let c = Config::parse("quick_key = \"lk=lock\"\nquick_key = \"gh=https://github.com/a?b=c\"\nquick_key = \"nonsense\"\nquick_key = \"=x\"\nquick_key = \"y=\"\n");
        assert_eq!(c.quick_keys, vec![("lk".to_string(), "lock".to_string()), ("gh".to_string(), "https://github.com/a?b=c".to_string())]);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn hidden_apps_roundtrip_once_each() {
        let mut c = Config::default();
        c.hide_app(r"shell:AppsFolder\{X}\old.exe");
        c.hide_app(r"SHELL:APPSFOLDER\{x}\OLD.exe");
        assert_eq!(c.apps_hidden.len(), 1);
        assert!(c.is_app_hidden(r"shell:appsfolder\{x}\old.exe"));
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn toggling_a_dock_pin_matches_by_key_ignoring_case_and_prefix() {
        let mut c = Config::default();
        assert!(c.toggle_dock_pin(r"shell:AppsFolder\Chrome"));
        assert!(!c.toggle_dock_pin(r"SHELL:appsfolder\CHROME"));
        assert!(c.dock_pins.is_empty());
    }
}

#[cfg(test)]
mod tray_tests {
    use super::*;

    #[test]
    fn tray_options_roundtrip() {
        let c = Config::parse("tray = \"off\"\ntray_icons = \"mono\"\ntray_pin = \"C:\\a\\steam.exe\"\ntray_pin = \"D:\\b.exe\"\n");
        assert!(c.tray_off && c.tray_mono);
        assert_eq!(c.tray_pins, vec![r"C:\a\steam.exe".to_string(), r"D:\b.exe".to_string()]);
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn tray_defaults_to_on_and_color() {
        let c = Config::parse("");
        assert!(!c.tray_off && !c.tray_mono && c.tray_pins.is_empty());
    }

    #[test]
    fn toggle_tray_pin_adds_then_removes_ignoring_case() {
        let mut c = Config::default();
        assert!(c.toggle_tray_pin(r"C:\Apps\Steam.exe"));
        assert!(!c.toggle_tray_pin(r"c:\apps\steam.EXE"));
        assert!(c.tray_pins.is_empty());
    }

    #[test]
    fn tray_pin_lookup_ignores_case() {
        let c = Config { tray_pins: vec![r"C:\Apps\Steam.exe".into()], ..Config::default() };
        assert!(c.is_tray_pinned(r"c:\apps\STEAM.exe"));
        assert!(!c.is_tray_pinned(r"c:\apps\other.exe"));
    }
}
