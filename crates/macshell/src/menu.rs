//! Содержимое меню логотипа и меню значка на панели. Рисует `ui/menu.slint`.

use crate::battery::{Hog, PowerInfo, info_lines};
use crate::clock::ClockOptions;
use crate::config::DockMode;
use crate::slint_view::MenuEntry;

pub const ID_SETTINGS: i32 = 0;
pub const ID_WIDGETS: i32 = 1;
pub const ID_RESTART: i32 = 2;
pub const ID_QUIT: i32 = 3;
pub const ID_APPS: i32 = 4;
pub const ID_PICK_LOGO: i32 = 10;
pub const ID_RESET_LOGO: i32 = 11;
pub const ID_ADD_APP: i32 = 12;
pub const ID_TRAY_STYLE: i32 = 13;
pub const ID_REMOVE_PIN: i32 = 20;
pub const ID_DOCK_PIN: i32 = 40;
pub const ID_DOCK_CLOSE: i32 = 41;
pub const ID_DOCK_SMART: i32 = 42;
pub const ID_DOCK_AUTOHIDE: i32 = 43;
pub const ID_DOCK_FIXED: i32 = 44;
pub const ID_ABOUT: i32 = 50;
pub const ID_SYSTEM_SETTINGS: i32 = 51;
pub const ID_STORE: i32 = 52;
pub const ID_RECENT: i32 = 53;
pub const ID_FORCE_QUIT: i32 = 54;
pub const ID_QUICK_KEYS: i32 = 55;
pub const ID_BATTERY_SAVER: i32 = 80;
pub const ID_BATTERY_PERCENT: i32 = 81;
pub const ID_BATTERY_SETTINGS: i32 = 82;
pub const ID_FOCUS_OFF: i32 = 97;
pub const ID_FOCUS_SETTINGS: i32 = 98;
pub const ID_WIDGET_RESET: i32 = 105;
pub const ID_MACSHELL_SETTINGS: i32 = 106;
pub const ID_MISSION: i32 = 107;
/// Виджеты в меню получают номера от этого: `ID_WIDGET_ITEM + номер вида`.
pub const ID_WIDGET_ITEM: i32 = 7000;
/// Меню самого виджета (правый клик): размеры от `ID_WIDGET_SIZE`, цвета от `ID_WIDGET_COLOR`, убрать.
pub const ID_WIDGET_SIZE: i32 = 7100;
pub const ID_WIDGET_COLOR: i32 = 7110;
pub const ID_WIDGET_REMOVE: i32 = 7130;
pub const ID_ITEM_LEFT: i32 = 99;
pub const ID_ITEM_RIGHT: i32 = 100;
pub const ID_ITEM_HIDE: i32 = 101;
pub const ID_BAR_AUTOHIDE: i32 = 102;
pub const ID_BAR_SOLID: i32 = 103;
pub const ID_BAR_RESET: i32 = 104;
/// Значки в меню панели получают номера от этого: `ID_BAR_ITEM + номер вида`.
pub const ID_BAR_ITEM: i32 = 6000;
pub const ID_APP_HIDE: i32 = 94;
pub const ID_APP_HIDE_OTHERS: i32 = 95;
pub const ID_APP_QUIT: i32 = 96;
pub const ID_WINDOW_LAYOUT_MENU: i32 = 8500;
pub const ID_WINDOW_LAYOUT_FIRST: i32 = 8510;
/// Пункты меню приложения получают номера от этого: `ID_APPMENU_ITEM + индекс в списке пунктов`.
pub const ID_APPMENU_ITEM: i32 = 5000;
pub const ID_APPMENU_MORE: i32 = 8000;
pub const ID_APPMENU_MORE_PREV: i32 = 7900;
pub const ID_APPMENU_MORE_NEXT: i32 = 7901;
pub const APP_MORE_PAGE: usize = 8;
pub const ID_CLOCK_SECONDS: i32 = 90;
pub const ID_CLOCK_12H: i32 = 91;
pub const ID_CLOCK_DATE: i32 = 92;
pub const ID_CLOCK_WEEKDAY: i32 = 93;
pub const ID_PRIVACY_MIC: i32 = 83;
pub const ID_PRIVACY_CAMERA: i32 = 84;
pub const ID_SLEEP: i32 = 60;
pub const ID_RESTART_PC: i32 = 61;
pub const ID_SHUTDOWN: i32 = 62;
pub const ID_LOCK: i32 = 63;
pub const ID_LOGOFF: i32 = 64;

pub const PADDING: f32 = 12.0;
pub const ROW: f32 = 28.0;
pub const SEPARATOR: f32 = 9.0;
pub const WIDTH: f32 = 270.0;

fn item(label: &str, id: i32, enabled: bool) -> MenuEntry {
    MenuEntry { label: label.into(), enabled, separator: false, id }
}

fn separator() -> MenuEntry {
    MenuEntry { label: "".into(), enabled: false, separator: true, id: -1 }
}

/// Меню Apple по клику на логотип: система сверху, питание ниже, настройки самой оболочки в конце.
/// «Стандартный логотип» доступен, только когда выбран свой; «Принудительно завершить» только когда есть что
/// завершать (активное окно не Проводник и не мы).
pub fn logo_menu(custom_logo: bool, tray_mono: bool, can_force_quit: bool) -> Vec<MenuEntry> {
    let tray_style = if tray_mono { "Вернуть цветные значки трея" } else { "Сделать значки трея одноцветными" };
    vec![
        item("Об этом компьютере", ID_ABOUT, true),
        separator(),
        item("Настройки macshell…", ID_MACSHELL_SETTINGS, true),
        item("Параметры системы…", ID_SYSTEM_SETTINGS, true),
        item("Магазин приложений…", ID_STORE, true),
        separator(),
        item("Приложения", ID_APPS, true),
        item("Mission Control", ID_MISSION, true),
        item("Недавние элементы", ID_RECENT, true),
        separator(),
        item("Принудительно завершить…", ID_FORCE_QUIT, can_force_quit),
        separator(),
        item("Сон", ID_SLEEP, true),
        item("Перезагрузить…", ID_RESTART_PC, true),
        item("Выключить…", ID_SHUTDOWN, true),
        separator(),
        item("Заблокировать экран", ID_LOCK, true),
        item("Выйти из учётной записи…", ID_LOGOFF, true),
        separator(),
        item("Выбрать логотип…", ID_PICK_LOGO, true),
        item("Стандартный логотип", ID_RESET_LOGO, custom_logo),
        item("Добавить приложение на панель…", ID_ADD_APP, true),
        item("Виджеты на рабочем столе…", ID_WIDGETS, true),
        item(tray_style, ID_TRAY_STYLE, true),
        item("Быстрые клавиши Spotlight…", ID_QUICK_KEYS, true),
        separator(),
        item("Перезапустить macshell", ID_RESTART, true),
        item("Выйти из macshell", ID_QUIT, true),
    ]
}

/// Меню правого клика по значку приложения на панели.
pub fn pin_menu() -> Vec<MenuEntry> {
    vec![item("Убрать с панели", ID_REMOVE_PIN, true)]
}

/// Меню батареи: сведения о питании, экономия заряда, проценты в панели и программы, нагружающие процессор.
/// Строки сведений и список программ только показывают, нажимаются остальные.
pub fn battery_menu(info: &PowerInfo, hogs: &[Hog], show_percent: bool) -> Vec<MenuEntry> {
    let mut m: Vec<MenuEntry> = info_lines(info).iter().map(|l| item(l, -1, false)).collect();
    m.push(separator());
    m.push(item(&format!("Экономия заряда: {}", if info.saver { "включена" } else { "выключена" }), ID_BATTERY_SAVER, true));
    m.push(item(if show_percent { "Скрыть проценты в панели" } else { "Показывать проценты в панели" }, ID_BATTERY_PERCENT, true));
    if !hogs.is_empty() {
        m.push(separator());
        m.push(item("Нагружают процессор", -1, false));
        for h in hogs {
            m.push(item(&format!("    {}   {} %", h.name, h.cpu.round().max(1.0) as i32), -1, false));
        }
    }
    m.push(separator());
    m.push(item("Параметры батареи…", ID_BATTERY_SETTINGS, true));
    m
}

/// Меню по клику на название приложения: скрыть, скрыть остальные, завершить. Без приложения (рабочий стол) всё серое.
pub fn app_menu(name: &str, has_app: bool) -> Vec<MenuEntry> {
    vec![
        item("Расположить окно…", ID_WINDOW_LAYOUT_MENU, has_app),
        separator(),
        item(&format!("Скрыть {name}"), ID_APP_HIDE, has_app),
        item("Скрыть остальные", ID_APP_HIDE_OTHERS, has_app),
        separator(),
        item(&format!("Завершить {name}"), ID_APP_QUIT, has_app),
    ]
}

pub fn window_layout_menu() -> Vec<MenuEntry> {
    ["Заполнить экран", "Левая половина", "Правая половина", "Верхняя половина", "Нижняя половина",
     "Вверху слева", "Вверху справа", "Внизу слева", "Внизу справа"]
        .iter().enumerate().map(|(i, label)| item(label, ID_WINDOW_LAYOUT_FIRST + i as i32, true)).collect()
}

/// Пункты одного из меню окна приложения. Включённые отмечены точкой, серые и заголовки вложенных меню не нажимаются.
/// Нажимаемые получают номер `ID_APPMENU_ITEM + индекс в items`.
pub fn app_items_menu(items: &[crate::appmenu::Item]) -> Vec<MenuEntry> {
    let any_checked = items.iter().any(|i| i.checked);
    items
        .iter()
        .enumerate()
        .map(|(k, i)| {
            if i.separator {
                return separator();
            }
            let mark = if i.checked { "• " } else if any_checked { "    " } else { "" };
            let clickable = i.enabled && i.command != 0;
            item(&format!("{mark}{}", i.label), if clickable { ID_APPMENU_ITEM + k as i32 } else { -1 }, clickable)
        })
        .collect()
}

pub fn app_more_menu(titles: &[String], start: usize, offset: usize) -> Vec<MenuEntry> {
    let mut rows = Vec::new();
    if offset > start { rows.push(item("‹ Предыдущие", ID_APPMENU_MORE_PREV, true)); }
    rows.extend(titles.iter().enumerate().skip(offset).take(APP_MORE_PAGE).map(|(i, title)| item(title, ID_APPMENU_MORE + i as i32, true)));
    if offset + APP_MORE_PAGE < titles.len() { rows.push(item("Следующие ›", ID_APPMENU_MORE_NEXT, true)); }
    rows
}

/// Меню виджетов: какие лежат на рабочем столе (с точкой) и возврат к начальным местам.
/// Меню виджета по правому клику: размер, цвет, убрать. Текущие значения отмечены точкой.
pub fn widget_menu(spec: &crate::widgets::Spec) -> Vec<MenuEntry> {
    let mark = |on: bool| if on { "• " } else { "    " };
    let mut m = vec![item(crate::widgets::title(spec.kind), -1, false), separator(), item("Размер", -1, false)];
    for (i, (label, _, _)) in crate::widgets::SIZES.iter().enumerate() {
        m.push(item(&format!("{}{label}", mark(spec.size == i)), ID_WIDGET_SIZE + i as i32, true));
    }
    m.push(separator());
    m.push(item("Цвет", -1, false));
    for (i, (label, _, _)) in crate::widgets::COLORS.iter().enumerate() {
        m.push(item(&format!("{}{label}", mark(spec.color == i)), ID_WIDGET_COLOR + i as i32, true));
    }
    m.push(separator());
    m.push(item("Убрать с рабочего стола", ID_WIDGET_REMOVE, true));
    m
}

pub fn widgets_menu(present: &[bool]) -> Vec<MenuEntry> {
    let mut m: Vec<MenuEntry> = (0..crate::widgets::KINDS.len())
        .map(|k| item(&format!("{}{}", if present[k] { "• " } else { "    " }, crate::widgets::title(k)), ID_WIDGET_ITEM + k as i32, true))
        .collect();
    m.push(separator());
    m.push(item("Расставить заново", ID_WIDGET_RESET, present.iter().any(|p| *p)));
    m
}

/// Правый клик по значку состояния: сдвинуть или скрыть. Сдвиг серый у края.
pub fn item_menu(kind: usize, can_left: bool, can_right: bool) -> Vec<MenuEntry> {
    vec![
        item("Сдвинуть влево", ID_ITEM_LEFT, can_left),
        item("Сдвинуть вправо", ID_ITEM_RIGHT, can_right),
        separator(),
        item(&format!("Скрыть «{}»", crate::baritems::title(kind)), ID_ITEM_HIDE, true),
    ]
}

/// Правый клик по пустому месту панели: какие значки показывать (с точкой те, что видны), автоскрытие, цвет.
/// `hidden`: скрытые виды, `active`: есть ли значок сейчас (без батареи значка батареи нет, пункт серый).
pub fn bar_menu(hidden: &[usize], active: &[bool; 10], autohide: bool, solid: bool) -> Vec<MenuEntry> {
    let mut m: Vec<MenuEntry> = (0..crate::baritems::KINDS.len())
        .map(|k| {
            let mark = if hidden.contains(&k) { "    " } else { "• " };
            item(&format!("{mark}{}", crate::baritems::title(k)), ID_BAR_ITEM + k as i32, active[k])
        })
        .collect();
    m.push(separator());
    m.push(item(&format!("{}Скрывать панель автоматически", if autohide { "• " } else { "    " }), ID_BAR_AUTOHIDE, true));
    m.push(item(&format!("{}Всегда сплошная панель", if solid { "• " } else { "    " }), ID_BAR_SOLID, true));
    m.push(separator());
    m.push(item("Вернуть порядок и значки", ID_BAR_RESET, true));
    m
}

/// Меню значка «Не беспокоить» в панели: выключить режим или открыть настройки уведомлений.
pub fn focus_menu() -> Vec<MenuEntry> {
    vec![item("Выключить «Не беспокоить»", ID_FOCUS_OFF, true), separator(), item("Параметры уведомлений…", ID_FOCUS_SETTINGS, true)]
}

/// Меню правого клика по часам: что показывать. Включённое отмечено точкой, как подключённая сеть.
pub fn clock_menu(o: &ClockOptions) -> Vec<MenuEntry> {
    let row = |on: bool, text: &str, id: i32| item(&format!("{}{text}", if on { "• " } else { "    " }), id, true);
    vec![
        row(!o.hide_weekday, "День недели", ID_CLOCK_WEEKDAY),
        row(!o.hide_date, "Дата", ID_CLOCK_DATE),
        row(o.seconds, "Секунды", ID_CLOCK_SECONDS),
        row(o.hour12, "12-часовой формат", ID_CLOCK_12H),
    ]
}

/// Меню точек приватности: кто сейчас пользуется микрофоном и камерой, и переход в их параметры.
pub fn privacy_menu(mic: &[String], camera: &[String]) -> Vec<MenuEntry> {
    let mut m = Vec::new();
    for (title, users) in [("Микрофон", mic), ("Камера", camera)] {
        if users.is_empty() {
            continue;
        }
        m.push(item(&format!("{title} используют"), -1, false));
        for u in users {
            m.push(item(&format!("    {u}"), -1, false));
        }
    }
    if !m.is_empty() {
        m.push(separator());
    }
    if !mic.is_empty() || mic.is_empty() && camera.is_empty() {
        m.push(item("Конфиденциальность: микрофон…", ID_PRIVACY_MIC, true));
    }
    if !camera.is_empty() || mic.is_empty() && camera.is_empty() {
        m.push(item("Конфиденциальность: камера…", ID_PRIVACY_CAMERA, true));
    }
    m
}

/// Меню значка Dock: закрепление, закрытие окон запущенного приложения и режим показа самого Dock.
pub fn dock_menu(name: &str, pinned: bool, running: bool, mode: DockMode) -> Vec<MenuEntry> {
    let mode_item = |label: &str, id: i32, this: DockMode| {
        let mark = if mode == this { "• " } else { "    " };
        item(&format!("{mark}{label}"), id, true)
    };
    vec![
        item(name, -1, false),
        separator(),
        item(if pinned { "Убрать из Dock" } else { "Оставить в Dock" }, ID_DOCK_PIN, true),
        item("Закрыть окна", ID_DOCK_CLOSE, running),
        separator(),
        mode_item("Прятать под окнами", ID_DOCK_SMART, DockMode::Smart),
        mode_item("Скрывать всегда", ID_DOCK_AUTOHIDE, DockMode::Autohide),
        mode_item("Занимать место", ID_DOCK_FIXED, DockMode::Fixed),
    ]
}

/// Высота окна меню в логических пикселях.
pub fn height(entries: &[MenuEntry]) -> f32 {
    PADDING + entries.iter().map(|e| if e.separator { SEPARATOR } else { ROW }).sum::<f32>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_menu_pages_through_every_hidden_title() {
        let titles: Vec<String> = (0..25).map(|i| format!("Меню {i}")).collect();
        let first = app_more_menu(&titles, 3, 3);
        assert_eq!(first.first().unwrap().id, ID_APPMENU_MORE + 3);
        assert_eq!(first.last().unwrap().id, ID_APPMENU_MORE_NEXT);
        let last = app_more_menu(&titles, 3, 19);
        assert_eq!(last.first().unwrap().id, ID_APPMENU_MORE_PREV);
        assert_eq!(last.last().unwrap().id, ID_APPMENU_MORE + 24);
    }

    fn ids(entries: &[MenuEntry]) -> Vec<i32> {
        entries.iter().filter(|e| !e.separator).map(|e| e.id).collect()
    }

    #[test]
    fn logo_menu_order_and_separators() {
        let m = logo_menu(true, false, true);
        assert_eq!(
            ids(&m),
            vec![
                ID_ABOUT,
                ID_MACSHELL_SETTINGS,
                ID_SYSTEM_SETTINGS, ID_STORE, ID_APPS, ID_MISSION, ID_RECENT, ID_FORCE_QUIT, ID_SLEEP, ID_RESTART_PC, ID_SHUTDOWN, ID_LOCK,
                ID_LOGOFF, ID_PICK_LOGO, ID_RESET_LOGO, ID_ADD_APP, ID_WIDGETS, ID_TRAY_STYLE, ID_QUICK_KEYS, ID_RESTART, ID_QUIT
            ]
        );
        assert_eq!(m.iter().filter(|e| e.separator).count(), 7);
    }

    #[test]
    fn reset_logo_needs_a_custom_logo_and_force_quit_needs_a_target() {
        let enabled = |m: &[MenuEntry], id| m.iter().find(|e| e.id == id).unwrap().enabled;
        assert!(enabled(&logo_menu(true, false, true), ID_RESET_LOGO));
        assert!(!enabled(&logo_menu(false, false, true), ID_RESET_LOGO));
        assert!(enabled(&logo_menu(false, false, true), ID_FORCE_QUIT));
        assert!(!enabled(&logo_menu(false, false, false), ID_FORCE_QUIT));
    }

    #[test]
    fn widget_menu_marks_the_current_size_and_color() {
        let spec = crate::widgets::Spec { size: 2, color: 3, ..crate::widgets::Spec::new(5, 0, 0) };
        let m = widget_menu(&spec);
        let marked: Vec<i32> = m.iter().filter(|e| e.label.to_string().starts_with("• ")).map(|e| e.id).collect();
        assert_eq!(marked, vec![ID_WIDGET_SIZE + 2, ID_WIDGET_COLOR + 3]);
        assert!(m.iter().any(|e| e.id == ID_WIDGET_REMOVE && e.enabled));
        assert_eq!(m.iter().filter(|e| e.id >= ID_WIDGET_COLOR && e.id < ID_WIDGET_REMOVE).count(), crate::widgets::COLORS.len());
        assert!(menu_ids_do_not_overlap());
    }

    fn menu_ids_do_not_overlap() -> bool {
        ID_WIDGET_ITEM + crate::widgets::KINDS.len() as i32 <= ID_WIDGET_SIZE
            && ID_WIDGET_SIZE + 3 <= ID_WIDGET_COLOR
            && ID_WIDGET_COLOR + crate::widgets::COLORS.len() as i32 <= ID_WIDGET_REMOVE
    }

    #[test]
    fn widgets_menu_marks_present_ones_and_resets_only_when_there_are_some() {
        let n = crate::widgets::KINDS.len();
        let mut present = vec![false; n];
        present[0] = true;
        present[2] = true;
        let m = widgets_menu(&present);
        let mut want: Vec<i32> = (0..n as i32).map(|k| ID_WIDGET_ITEM + k).collect();
        want.push(ID_WIDGET_RESET);
        assert_eq!(ids(&m), want);
        assert!(m[0].label.to_string().starts_with("• ") && m[1].label.to_string().starts_with("    "));
        assert!(m[n + 1].enabled && !widgets_menu(&vec![false; n])[n + 1].enabled);
    }

    #[test]
    fn bar_menus_offer_every_icon_and_mark_the_visible_ones() {
        let mut active = [true; 10];
        active[0] = false;
        let m = bar_menu(&[3], &active, true, false);
        assert_eq!(ids(&m).len(), 10 + 3);
        assert!(m[1].label.to_string().starts_with("• ") && m[3].label.to_string().starts_with("    "));
        assert!(!m[0].enabled, "battery row is grey without a battery");
        assert!(m.iter().any(|e| e.label.to_string().starts_with("• Скрывать")));
        let item = item_menu(7, false, true);
        assert_eq!(ids(&item), vec![ID_ITEM_LEFT, ID_ITEM_RIGHT, ID_ITEM_HIDE]);
        assert!(!item[0].enabled && item[1].enabled && item[3].label.to_string().contains("Сеть"));
    }

    #[test]
    fn app_menu_is_greyed_without_an_app() {
        let m = app_menu("Telegram", true);
        assert_eq!(ids(&m), vec![ID_WINDOW_LAYOUT_MENU, ID_APP_HIDE, ID_APP_HIDE_OTHERS, ID_APP_QUIT]);
        assert!(m.iter().any(|e| e.label.to_string().contains("Telegram")) && m.iter().all(|e| e.separator || e.enabled));
        assert!(app_menu("Рабочий стол", false).iter().all(|e| !e.enabled));
    }

    #[test]
    fn app_items_get_numbers_by_position_and_greyed_ones_do_not_click() {
        use crate::appmenu::Item;
        let it = |label: &str, command, enabled, checked| Item { label: label.into(), command, enabled, checked, separator: false };
        let items = vec![
            it("New", 101, true, true),
            Item { label: String::new(), command: 0, enabled: false, checked: false, separator: true },
            it("Exit", 102, false, false),
            it("More ›", 0, false, false),
            it("    Deep", 301, true, false),
        ];
        let m = app_items_menu(&items);
        assert_eq!(ids(&m), vec![ID_APPMENU_ITEM, -1, -1, ID_APPMENU_ITEM + 4]);
        assert!(m[0].label.to_string().starts_with("• New"));
        assert!(m[2].label.to_string().starts_with("    Exit"), "unchecked rows line up with checked ones");
        assert!(!m[2].enabled);
    }

    #[test]
    fn clock_menu_marks_what_is_shown() {
        let m = clock_menu(&ClockOptions { seconds: true, ..Default::default() });
        assert_eq!(ids(&m), vec![ID_CLOCK_WEEKDAY, ID_CLOCK_DATE, ID_CLOCK_SECONDS, ID_CLOCK_12H]);
        let marked: Vec<bool> = m.iter().map(|e| e.label.to_string().starts_with('•')).collect();
        assert_eq!(marked, vec![true, true, true, false]);
    }

    #[test]
    fn privacy_menu_lists_users_and_links_to_their_settings() {
        let m = privacy_menu(&["Zoom".to_string()], &[]);
        assert_eq!(ids(&m), vec![-1, -1, ID_PRIVACY_MIC], "separators are not listed by ids()");
        assert!(m[1].label.to_string().contains("Zoom"));
        let both = privacy_menu(&["Zoom".to_string()], &["Teams".to_string()]);
        assert_eq!(ids(&both), vec![-1, -1, -1, -1, ID_PRIVACY_MIC, ID_PRIVACY_CAMERA]);
        assert_eq!(ids(&privacy_menu(&[], &[])), vec![ID_PRIVACY_MIC, ID_PRIVACY_CAMERA]);
    }

    #[test]
    fn battery_menu_shows_facts_then_switches_then_hogs() {
        let info = crate::battery::info_from(0, 1, 64, 8100, 1);
        let hogs = vec![Hog { name: "chrome".into(), cpu: 7.6 }, Hog { name: "code".into(), cpu: 0.6 }];
        let m = battery_menu(&info, &hogs, false);
        assert_eq!(ids(&m), vec![-1, -1, -1, ID_BATTERY_SAVER, ID_BATTERY_PERCENT, -1, -1, -1, ID_BATTERY_SETTINGS]);
        assert_eq!(m[0].label.to_string(), "Заряд: 64 %");
        assert!(m.iter().find(|e| e.id == ID_BATTERY_SAVER).unwrap().label.to_string().ends_with("включена"));
        assert!(m.iter().any(|e| e.label.to_string().trim() == "chrome   8 %"));
        assert!(m.iter().any(|e| e.label.to_string().trim() == "code   1 %"), "never shows 0 %");
        assert!(m.iter().filter(|e| e.id == -1 && !e.separator).all(|e| !e.enabled), "facts are not buttons");
        let quiet = battery_menu(&info, &[], true);
        assert!(!quiet.iter().any(|e| e.label.to_string().contains("Нагружают")));
        assert_eq!(quiet.iter().find(|e| e.id == ID_BATTERY_PERCENT).unwrap().label.to_string(), "Скрыть проценты в панели");
    }

    #[test]
    fn dock_menu_follows_the_item_and_the_mode() {
        let m = dock_menu("Steam", true, true, DockMode::Smart);
        assert_eq!(ids(&m), vec![-1, ID_DOCK_PIN, ID_DOCK_CLOSE, ID_DOCK_SMART, ID_DOCK_AUTOHIDE, ID_DOCK_FIXED]);
        assert_eq!(m[2].label.to_string(), "Убрать из Dock");
        assert!(!m[0].enabled, "the name is a caption, not a command");
        let idle = dock_menu("Steam", false, false, DockMode::Autohide);
        assert_eq!(idle[2].label.to_string(), "Оставить в Dock");
        assert!(!idle.iter().find(|e| e.id == ID_DOCK_CLOSE).unwrap().enabled, "nothing to close");
        let marked: Vec<i32> = idle.iter().filter(|e| e.label.to_string().starts_with('•')).map(|e| e.id).collect();
        assert_eq!(marked, vec![ID_DOCK_AUTOHIDE], "only the current mode is ticked");
    }

    #[test]
    fn height_adds_rows_separators_and_padding() {
        assert_eq!(height(&pin_menu()), 12.0 + 28.0);
        let m = logo_menu(false, false, true);
        assert_eq!(height(&m), 12.0 + 21.0 * 28.0 + 7.0 * 9.0);
    }
}
