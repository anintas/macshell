//! Вкладка «Действия» в Spotlight: системные команды без открытия приложений (блокировка, сон, тёмная тема,
//! Bluetooth, корзина, снимок экрана и так далее). Найденное действие выполняется по Enter.

use crate::search::{match_score, normalize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionId {
    Lock,
    Sleep,
    Restart,
    Shutdown,
    LogOff,
    DarkMode,
    Bluetooth,
    Mute,
    Screenshot,
    TaskManager,
    EmptyRecycleBin,
    NightLight,
    Terminal,
    ClearClipboard,
    Mission,
    ShowDesktop,
    Panel,
    Calendar,
    MacshellSettings,
    Stage,
}

#[derive(Debug)]
pub struct ActionDef {
    pub id: ActionId,
    /// Короткое имя для конфига: `quick_key = "lk=lock"`.
    pub name: &'static str,
    pub title: &'static str,
    /// Слова поиска на двух языках через запятую.
    pub keywords: &'static str,
    /// Имя значка в `spotlight.slint` (`ResultRow.glyph-path`).
    pub glyph: &'static str,
}

/// Порядок каталога = порядок при пустом запросе.
pub const CATALOG: &[ActionDef] = &[
    ActionDef { id: ActionId::Lock, name: "lock", title: "Заблокировать экран", keywords: "lock,screen,блокировка,заблокировать", glyph: "lock" },
    ActionDef { id: ActionId::Sleep, name: "sleep", title: "Сон", keywords: "sleep,suspend,спящий режим", glyph: "power" },
    ActionDef { id: ActionId::DarkMode, name: "dark", title: "Переключить тёмную тему", keywords: "dark,light,theme,mode,тема,светлая,тёмная", glyph: "moon" },
    ActionDef { id: ActionId::Bluetooth, name: "bluetooth", title: "Включить или выключить Bluetooth", keywords: "bluetooth,блютуз", glyph: "bluetooth" },
    ActionDef { id: ActionId::Mute, name: "mute", title: "Выключить или включить звук", keywords: "mute,sound,volume,звук,без звука", glyph: "speaker" },
    ActionDef { id: ActionId::Screenshot, name: "screenshot", title: "Снимок экрана", keywords: "screenshot,screen clip,snip,скриншот,снимок", glyph: "camera" },
    ActionDef { id: ActionId::EmptyRecycleBin, name: "trash", title: "Очистить корзину", keywords: "recycle,bin,trash,empty,корзина", glyph: "trash" },
    ActionDef { id: ActionId::TaskManager, name: "taskmgr", title: "Диспетчер задач", keywords: "task manager,processes,taskmgr,процессы", glyph: "monitor" },
    ActionDef { id: ActionId::Terminal, name: "terminal", title: "Терминал", keywords: "terminal,console,cmd,powershell,консоль,командная строка", glyph: "terminal" },
    ActionDef { id: ActionId::NightLight, name: "nightlight", title: "Ночной свет", keywords: "night light,blue light,ночной режим", glyph: "moon" },
    ActionDef { id: ActionId::ClearClipboard, name: "clearclip", title: "Очистить историю буфера обмена", keywords: "clipboard,history,clear,буфер,история", glyph: "clipboard" },
    ActionDef { id: ActionId::Mission, name: "mission", title: "Mission Control", keywords: "mission control,overview,windows,exposé,обзор окон,все окна", glyph: "monitor" },
    ActionDef { id: ActionId::ShowDesktop, name: "desktop", title: "Показать рабочий стол", keywords: "show desktop,minimize all,рабочий стол,свернуть все", glyph: "monitor" },
    ActionDef { id: ActionId::Panel, name: "panel", title: "Центр уведомлений и виджеты", keywords: "notification center,widgets,уведомления,виджеты", glyph: "clipboard" },
    ActionDef { id: ActionId::Calendar, name: "calendar", title: "Управлять календарём", keywords: "calendar,календарь,месяц,дата", glyph: "clipboard" },
    ActionDef { id: ActionId::MacshellSettings, name: "settings", title: "Настройки macshell", keywords: "settings,preferences,macshell,настройки", glyph: "gear" },
    ActionDef { id: ActionId::Stage, name: "stage", title: "Stage Manager", keywords: "stage manager,focus,windows,сцена,диспетчер сцены", glyph: "monitor" },
    ActionDef { id: ActionId::LogOff, name: "logoff", title: "Выйти из учётной записи", keywords: "log off,sign out,logout,выход", glyph: "power" },
    ActionDef { id: ActionId::Restart, name: "restart", title: "Перезагрузить", keywords: "restart,reboot,перезагрузка", glyph: "power" },
    ActionDef { id: ActionId::Shutdown, name: "shutdown", title: "Выключить компьютер", keywords: "shutdown,power off,выключение,выключить", glyph: "power" },
];

/// Действие по короткому имени из конфига (без учёта регистра).
pub fn by_name(name: &str) -> Option<&'static ActionDef> {
    let n = name.trim();
    CATALOG.iter().find(|a| a.name.eq_ignore_ascii_case(n))
}

/// Описание действия по его номеру.
pub fn def_of(id: ActionId) -> Option<&'static ActionDef> {
    CATALOG.iter().find(|a| a.id == id)
}

/// Действия под запрос: пустой запрос даёт начало каталога, иначе лучшие совпадения (до `limit`).
pub fn search(query: &str, limit: usize) -> Vec<&'static ActionDef> {
    if normalize(query).is_empty() {
        return CATALOG.iter().take(limit).collect();
    }
    let mut hits: Vec<(i32, &ActionDef)> = CATALOG
        .iter()
        .filter_map(|a| {
            let keywords: Vec<String> = a.keywords.split(',').map(normalize).collect();
            Some((match_score(query, &normalize(a.title), &keywords)?, a))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().take(limit).map(|(_, a)| a).collect()
}

/// Выполняет действие. Опасные (перезагрузка, выключение, выход) спрашивают подтверждение сами.
pub fn run(id: ActionId) {
    use crate::power::{self, PowerAction};
    match id {
        ActionId::Lock => power::run(PowerAction::Lock),
        ActionId::Sleep => power::run(PowerAction::Sleep),
        ActionId::Restart => power::run(PowerAction::Restart),
        ActionId::Shutdown => power::run(PowerAction::Shutdown),
        ActionId::LogOff => power::run(PowerAction::LogOff),
        ActionId::DarkMode => crate::theme::set_dark(!crate::theme::is_dark()),
        ActionId::Bluetooth => {
            let state = crate::bluetooth::state();
            if state != crate::bluetooth::NONE {
                crate::bluetooth::set_enabled(state != crate::bluetooth::ON);
            }
        }
        ActionId::Mute => {
            if let Some(v) = crate::status::VolumeControl::open() {
                v.set_muted(!v.muted());
            }
        }
        ActionId::Screenshot => crate::bar::launch("ms-screenclip:"),
        ActionId::TaskManager => crate::bar::launch("taskmgr.exe"),
        ActionId::Terminal => crate::bar::launch("wt.exe"),
        ActionId::NightLight => crate::bar::launch("ms-settings:nightlight"),
        ActionId::EmptyRecycleBin => empty_recycle_bin(),
        ActionId::ClearClipboard => crate::clipboard::clear(),
        ActionId::Mission => crate::requests::set(crate::requests::Request::Mission),
        ActionId::ShowDesktop => {
            crate::desktop::toggle();
        }
        ActionId::Panel => crate::requests::set(crate::requests::Request::Panel),
        ActionId::Calendar => crate::requests::set(crate::requests::Request::Calendar),
        ActionId::Stage => crate::requests::set(crate::requests::Request::Stage),
        ActionId::MacshellSettings => crate::requests::set(crate::requests::Request::Settings),
    }
}

/// Очистка корзины системным способом: Windows сама спрашивает подтверждение и показывает ход.
fn empty_recycle_bin() {
    use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHERB_NOSOUND};
    unsafe {
        let _ = SHEmptyRecycleBinW(None, None, SHERB_NOSOUND);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&ActionDef]) -> Vec<ActionId> {
        v.iter().map(|a| a.id).collect()
    }

    #[test]
    fn empty_query_lists_the_start_of_the_catalog() {
        let list = search("", 7);
        assert_eq!(list.len(), 7);
        assert_eq!(list[0].id, ActionId::Lock);
        assert_eq!(ids(&search("  ", 3)), vec![ActionId::Lock, ActionId::Sleep, ActionId::DarkMode]);
    }

    #[test]
    fn russian_and_english_words_find_the_action() {
        assert_eq!(search("блок", 3)[0].id, ActionId::Lock);
        assert_eq!(search("lock", 3)[0].id, ActionId::Lock);
        assert_eq!(search("reboot", 3)[0].id, ActionId::Restart);
        assert_eq!(search("корзин", 3)[0].id, ActionId::EmptyRecycleBin);
        assert_eq!(search("bluetooth", 3)[0].id, ActionId::Bluetooth);
        assert!(search("qwertyuiop", 3).is_empty());
    }

    #[test]
    fn query_typed_in_the_wrong_layout_still_finds_it() {
        // «блок» набрано в английской раскладке.
        assert_eq!(search(",kjr", 3)[0].id, ActionId::Lock);
    }

    #[test]
    fn config_names_find_actions_and_are_unique() {
        assert_eq!(by_name("Lock").map(|a| a.id), Some(ActionId::Lock));
        assert_eq!(by_name(" dark ").map(|a| a.id), Some(ActionId::DarkMode));
        assert!(by_name("nope").is_none());
        let mut seen = Vec::new();
        for a in CATALOG {
            assert!(!seen.contains(&a.name), "duplicate name {}", a.name);
            seen.push(a.name);
            assert_eq!(def_of(a.id).map(|d| d.name), Some(a.name));
        }
    }

    #[test]
    fn every_action_has_a_title_a_glyph_and_a_unique_id() {
        let mut seen = Vec::new();
        for a in CATALOG {
            assert!(!a.title.is_empty() && !a.glyph.is_empty() && !a.keywords.is_empty());
            assert!(!seen.contains(&a.id), "duplicate {:?}", a.id);
            seen.push(a.id);
        }
    }
}
