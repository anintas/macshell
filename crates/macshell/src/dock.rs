//! Модель Dock: закреплённые приложения и запущенные окна сливаются в один ряд значков.
//! Закреплённые идут первыми в порядке закрепления, остальные запущенные после них в порядке появления.

/// Окно приложения, которое показывается в Dock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppWindow {
    pub hwnd: isize,
    /// Одно и то же приложение у всех его окон: AUMID или путь exe в нижнем регистре.
    pub key: String,
    /// Чем запускать приложение: `shell:AppsFolder\AUMID` или путь exe.
    pub target: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockItem {
    pub key: String,
    pub target: String,
    pub name: String,
    pub pinned: bool,
    /// Окна приложения в порядке опроса; пусто, если приложение не запущено.
    pub hwnds: Vec<isize>,
}

impl DockItem {
    pub fn running(&self) -> bool {
        !self.hwnds.is_empty()
    }
}

const APPS_FOLDER: &str = "shell:appsfolder\\";

/// Ключ закреплённого приложения: так же, как у окон (`AppWindow::key`).
pub fn key_of(target: &str) -> String {
    let lower = target.trim().to_lowercase();
    lower.strip_prefix(APPS_FOLDER).map(str::to_string).unwrap_or(lower)
}

/// Цель запуска по AUMID.
pub fn target_for_aumid(aumid: &str) -> String {
    format!("shell:AppsFolder\\{aumid}")
}

fn display_name(target: &str, window_title: Option<&str>) -> String {
    let stem = target.rsplit(['\\', '/', '!']).next().unwrap_or(target);
    let stem = stem.rsplit_once('.').filter(|(_, ext)| ext.len() <= 4).map(|(s, _)| s).unwrap_or(stem);
    match window_title {
        Some(t) if target.to_lowercase().starts_with(APPS_FOLDER) && !t.is_empty() => t.to_string(),
        _ => stem.to_string(),
    }
}

/// Собирает ряд значков. `order` помнит порядок появления незакреплённых приложений между вызовами:
/// значки не прыгают, когда окна открываются и закрываются.
pub fn merge(pins: &[String], windows: &[AppWindow], order: &mut Vec<String>) -> Vec<DockItem> {
    let mut items: Vec<DockItem> = Vec::new();
    for pin in pins {
        let key = key_of(pin);
        if items.iter().any(|i| i.key == key) {
            continue;
        }
        items.push(DockItem { name: display_name(pin, None), key, target: pin.clone(), pinned: true, hwnds: Vec::new() });
    }
    for w in windows {
        if let Some(item) = items.iter_mut().find(|i| i.key == w.key) {
            item.hwnds.push(w.hwnd);
        }
    }
    // Незакреплённые: сначала уже известные в прежнем порядке, потом новые.
    let unpinned: Vec<&AppWindow> = windows.iter().filter(|w| !items.iter().any(|i| i.pinned && i.key == w.key)).collect();
    order.retain(|k| unpinned.iter().any(|w| &w.key == k));
    for w in &unpinned {
        if !order.contains(&w.key) {
            order.push(w.key.clone());
        }
    }
    for key in order.iter() {
        let group: Vec<&&AppWindow> = unpinned.iter().filter(|w| &w.key == key).collect();
        let first = group[0];
        items.push(DockItem {
            key: key.clone(),
            target: first.target.clone(),
            name: display_name(&first.target, Some(&first.title)),
            pinned: false,
            hwnds: group.iter().map(|w| w.hwnd).collect(),
        });
    }
    items
}

/// Что стоит в ряду Dock: кнопка «Приложения», значок приложения (номер в `DockItem`) или разделитель.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotRef {
    Launcher,
    Item(usize),
    Separator,
    Downloads,
    Trash,
}

/// Раскладка ряда: кнопка «Приложения», разделитель, закреплённые, разделитель, остальные запущенные.
/// Разделитель стоит, только если по обе стороны от него что-то есть.
pub fn slots_for(items: &[DockItem]) -> Vec<SlotRef> {
    let mut slots = vec![SlotRef::Launcher];
    let pinned: Vec<usize> = (0..items.len()).filter(|&i| items[i].pinned).collect();
    let others: Vec<usize> = (0..items.len()).filter(|&i| !items[i].pinned).collect();
    if !items.is_empty() {
        slots.push(SlotRef::Separator);
    }
    slots.extend(pinned.iter().map(|&i| SlotRef::Item(i)));
    if !pinned.is_empty() && !others.is_empty() {
        slots.push(SlotRef::Separator);
    }
    slots.extend(others.iter().map(|&i| SlotRef::Item(i)));
    slots
}

/// Ширина Dock в логических пикселях для `count` значков и `separators` разделителей.
pub fn width_pt(count: usize, separators: usize) -> f32 {
    PADDING_PT * 2.0 + count as f32 * SLOT_PT + separators as f32 * SEPARATOR_PT
}

/// Уменьшает ряд целиком, если он не помещается на экран; порядок и разделители сохраняются.
pub fn fit_factor(slots: &[SlotRef], screen_width_pt: f32) -> f32 {
    let separators = slots.iter().filter(|s| **s == SlotRef::Separator).count();
    let width = width_pt(slots.len() - separators, separators);
    ((screen_width_pt - 8.0).max(1.0) / width).min(1.0)
}

/// Центр и размер значка в физических пикселях для перехода окна к Dock.
pub fn motion_target(slots: &[SlotRef], item: usize, screen_width_pt: f32, scale: f32) -> Option<(f32, f32)> {
    let fit = fit_factor(slots, screen_width_pt);
    let separators = slots.iter().filter(|s| **s == SlotRef::Separator).count();
    let total = width_pt(slots.len() - separators, separators) * fit;
    let mut x = (screen_width_pt - total) / 2.0 + PADDING_PT * fit;
    for slot in slots {
        let width = if *slot == SlotRef::Separator { SEPARATOR_PT } else { SLOT_PT } * fit;
        if *slot == SlotRef::Item(item) {
            return Some(((x + width / 2.0) * scale, 48.0 * fit * scale));
        }
        x += width;
    }
    None
}

pub const SLOT_PT: f32 = 60.0;
pub const SEPARATOR_PT: f32 = 14.0;
pub const PADDING_PT: f32 = 10.0;
/// Высота окна Dock: значок 48, поля, точка «запущено» под ним.
pub const HEIGHT_PT: f32 = 72.0;
/// Место над подложкой для подписи при наведении; не резервируется для окон.
pub const LABEL_HEIGHT_PT: f32 = 30.0;
/// Зазор между Dock и нижним краем экрана.
pub const MARGIN_PT: f32 = 8.0;

/// Какой значок под точкой `x` (логические пиксели от левого края окна). Слот: значок и поле вокруг него.
pub fn slot_at(x: f32, count: usize, separators_after: &[usize]) -> Option<usize> {
    let mut left = PADDING_PT;
    for i in 0..count {
        if x >= left && x < left + SLOT_PT {
            return Some(i);
        }
        left += SLOT_PT;
        if separators_after.contains(&i) {
            left += SEPARATOR_PT;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_target_tracks_the_app_slot_and_scaled_separators() {
        let slots = [SlotRef::Launcher, SlotRef::Separator, SlotRef::Item(0), SlotRef::Item(1), SlotRef::Separator, SlotRef::Trash];
        let first = motion_target(&slots, 0, 1920.0, 1.0).unwrap();
        let second = motion_target(&slots, 1, 1920.0, 1.0).unwrap();
        assert!((second.0 - first.0 - 60.0).abs() < 0.01);
        assert!((first.0 - 930.0).abs() < 0.01, "unexpected icon center {first:?}");
        assert_eq!(first.1, 48.0);
        assert!(motion_target(&slots, 9, 1920.0, 1.0).is_none());
        let compact = motion_target(&slots, 0, 200.0, 1.25).unwrap();
        assert!(compact.0 >= 0.0 && compact.0 <= 250.0);
        assert!(compact.1 < 48.0 * 1.25);
    }

    #[test]
    fn long_dock_fits_narrow_screens_without_reordering() {
        let slots = vec![SlotRef::Launcher, SlotRef::Separator, SlotRef::Item(0), SlotRef::Item(1), SlotRef::Item(2), SlotRef::Item(3), SlotRef::Separator, SlotRef::Downloads, SlotRef::Trash];
        let factor = fit_factor(&slots, 320.0);
        assert!(factor < 1.0);
        assert!(width_pt(7, 2) * factor <= 312.0 + f32::EPSILON);
        assert_eq!(fit_factor(&slots, 1920.0), 1.0);
    }

    fn win(hwnd: isize, key: &str, title: &str) -> AppWindow {
        AppWindow { hwnd, key: key.into(), target: format!("C:\\Apps\\{key}.exe"), title: title.into() }
    }

    fn keys(items: &[DockItem]) -> Vec<&str> {
        items.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn pins_come_first_in_their_own_order_even_when_not_running() {
        let pins = vec![r"C:\Apps\b.exe".to_string(), r"C:\Apps\a.exe".to_string()];
        let items = merge(&pins, &[], &mut Vec::new());
        assert_eq!(keys(&items), vec![r"c:\apps\b.exe", r"c:\apps\a.exe"]);
        assert!(items.iter().all(|i| i.pinned && !i.running()));
    }

    #[test]
    fn windows_attach_to_their_pin_and_the_rest_follow_in_order_of_appearance() {
        let pins = vec![r"C:\Apps\a.exe".to_string()];
        let windows = vec![win(1, r"c:\apps\z.exe", "Z"), win(2, r"c:\apps\a.exe", "A one"), win(3, r"c:\apps\a.exe", "A two"), win(4, r"c:\apps\m.exe", "M")];
        let mut order = Vec::new();
        let items = merge(&pins, &windows, &mut order);
        assert_eq!(keys(&items), vec![r"c:\apps\a.exe", r"c:\apps\z.exe", r"c:\apps\m.exe"]);
        assert_eq!(items[0].hwnds, vec![2, 3]);
        assert!(items[0].pinned && items[0].running());
        assert!(!items[1].pinned);
    }

    #[test]
    fn unpinned_order_is_stable_across_calls_and_forgets_closed_apps() {
        let mut order = Vec::new();
        let first = vec![win(1, "z", "Z"), win(2, "m", "M")];
        assert_eq!(keys(&merge(&[], &first, &mut order)), vec!["z", "m"]);
        // Окна пришли в другом порядке, значки остались на местах.
        let swapped = vec![win(2, "m", "M"), win(1, "z", "Z"), win(5, "k", "K")];
        assert_eq!(keys(&merge(&[], &swapped, &mut order)), vec!["z", "m", "k"]);
        // z закрыт: исчез; вернулся позже: встал в конец.
        let without_z = vec![win(2, "m", "M"), win(5, "k", "K")];
        assert_eq!(keys(&merge(&[], &without_z, &mut order)), vec!["m", "k"]);
        let back = vec![win(2, "m", "M"), win(5, "k", "K"), win(9, "z", "Z")];
        assert_eq!(keys(&merge(&[], &back, &mut order)), vec!["m", "k", "z"]);
    }

    #[test]
    fn duplicate_pins_collapse_and_aumid_pins_match_window_keys() {
        let pins = vec!["shell:AppsFolder\\Chrome".to_string(), "shell:appsfolder\\CHROME".to_string()];
        let w = AppWindow { hwnd: 7, key: "chrome".into(), target: target_for_aumid("Chrome"), title: "Google".into() };
        let items = merge(&pins, &[w], &mut Vec::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].hwnds, vec![7]);
    }

    #[test]
    fn key_of_ignores_case_and_the_apps_folder_prefix() {
        assert_eq!(key_of("shell:AppsFolder\\Microsoft.Paint_8wekyb3d8bbwe!App"), "microsoft.paint_8wekyb3d8bbwe!app");
        assert_eq!(key_of(r"C:\Apps\Foo.EXE"), r"c:\apps\foo.exe");
    }

    #[test]
    fn names_come_from_the_file_or_the_window_title_for_packaged_apps() {
        assert_eq!(display_name(r"C:\Apps\Foo Bar.exe", Some("title")), "Foo Bar");
        assert_eq!(display_name("shell:AppsFolder\\Microsoft.Paint_8wekyb3d8bbwe!App", Some("Paint")), "Paint");
        assert_eq!(display_name("shell:AppsFolder\\Chrome", None), "Chrome");
    }

    fn item(key: &str, pinned: bool) -> DockItem {
        DockItem { key: key.into(), target: key.into(), name: key.into(), pinned, hwnds: vec![] }
    }

    #[test]
    fn slots_have_separators_only_between_groups() {
        use SlotRef::*;
        assert_eq!(slots_for(&[]), vec![Launcher]);
        assert_eq!(slots_for(&[item("a", true)]), vec![Launcher, Separator, Item(0)]);
        assert_eq!(slots_for(&[item("a", false)]), vec![Launcher, Separator, Item(0)]);
        let both = slots_for(&[item("a", true), item("b", true), item("c", false)]);
        assert_eq!(both, vec![Launcher, Separator, Item(0), Item(1), Separator, Item(2)]);
    }

    #[test]
    fn width_and_slot_hit_testing_agree() {
        assert_eq!(width_pt(3, 1), 20.0 + 180.0 + 14.0);
        // Три значка, разделитель после второго (номер 1).
        assert_eq!(slot_at(5.0, 3, &[1]), None);
        assert_eq!(slot_at(10.0, 3, &[1]), Some(0));
        assert_eq!(slot_at(69.9, 3, &[1]), Some(0));
        assert_eq!(slot_at(70.0, 3, &[1]), Some(1));
        assert_eq!(slot_at(135.0, 3, &[1]), None, "разделитель не слот");
        assert_eq!(slot_at(144.0, 3, &[1]), Some(2));
        assert_eq!(slot_at(500.0, 3, &[1]), None);
    }
}
