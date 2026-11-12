//! Окно настроек macshell: вкладки со строками (значки панели, часы, Dock, виджеты, клавиши и горячие углы,
//! цвета). Модель строк и применение значений к конфигу это чистые функции, окно только показывает их и передаёт
//! щелчки. Каждое изменение сразу пишется в конфиг; окно при закрытии сообщает, менялось ли что-то: тогда
//! macshell перезапускается и все окна строятся с новыми настройками.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use anyhow::Result;
use capture::RectI;
use glass::{Bitmap, cpu};
use slint::{Color, Model, ModelRc, SharedString, VecModel};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::bar::popup_glass;
use crate::baritems;
use crate::config::{Config, DockMode};
use crate::host::{InputEvent, LayeredWindow, primary_scale};
use crate::popups::popup_glass_params;
use crate::slint_view::{ContentView, SetRow, Settings};
use crate::widgets;

const WIDTH_PT: f32 = 720.0;
const HEIGHT_PT: f32 = 500.0;
const VK_ESCAPE: u32 = 0x1B;

pub const TABS: [&str; 6] = ["Значки", "Панель", "Dock", "Виджеты", "Клавиши", "Цвета"];

/// Что делает курсор, ушедший в угол экрана: значение в конфиге и подпись.
pub const CORNER_ACTIONS: [(&str, &str); 8] = [
    ("", "Ничего"),
    ("mission", "Mission Control"),
    ("apps", "Приложения"),
    ("spotlight", "Spotlight"),
    ("desktop", "Показать рабочий стол"),
    ("lock", "Заблокировать экран"),
    ("notifications", "Центр уведомлений"),
    ("stage", "Stage Manager"),
];

const CORNER_NAMES: [&str; 4] = ["Верхний левый угол", "Верхний правый угол", "Нижний левый угол", "Нижний правый угол"];

/// Цвета системы: подпись и цвет.
pub const ACCENTS: [(&str, [u8; 3]); 8] = [
    ("Синий", [0x46, 0x73, 0xE2]),
    ("Фиолетовый", [0x8E, 0x5C, 0xF0]),
    ("Розовый", [0xE2, 0x56, 0x9B]),
    ("Красный", [0xE2, 0x57, 0x4C]),
    ("Оранжевый", [0xE8, 0x89, 0x2F]),
    ("Жёлтый", [0xD9, 0xB6, 0x3A]),
    ("Зелёный", [0x3F, 0xAE, 0x5F]),
    ("Графит", [0x8A, 0x8F, 0x9B]),
];

/// Цвета сплошной панели.
pub const BAR_COLORS: [(&str, [u8; 3]); 6] = [
    ("Чёрный", [0, 0, 0]),
    ("Графит", [0x1C, 0x1C, 0x1E]),
    ("Тёмно-синий", [0x0B, 0x1A, 0x33]),
    ("Тёмно-фиолетовый", [0x1E, 0x12, 0x33]),
    ("Тёмно-зелёный", [0x0E, 0x2A, 0x1E]),
    ("Тёмно-красный", [0x2E, 0x10, 0x10]),
];

const DOCK_MODES: [(DockMode, &str); 4] = [
    (DockMode::Smart, "Умный"),
    (DockMode::Autohide, "Автоскрытие"),
    (DockMode::Fixed, "Всегда на виду"),
    (DockMode::Off, "Выключен"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    BarItem(usize),
    BarAutohide,
    BarSolid,
    BatteryPercent,
    ClockSeconds,
    Clock12h,
    ClockDate,
    ClockWeekday,
    DockMode,
    Stage,
    Widget(usize),
    WinTap,
    Switcher,
    SpotlightKey,
    Corner(usize),
    Accent,
    BarColor,
    /// Цвет всех виджетов сразу (у каждого отдельно: правый клик по виджету).
    WidgetsColor,
    GlassDensity,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Toggle(bool),
    /// Значение по кругу: варианты и номер выбранного.
    Cycle(Vec<String>, usize),
    Swatches(Vec<[u8; 3]>, Option<usize>),
    Slider(u8),
    Heading,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: Option<Id>,
    pub label: String,
    pub hint: String,
    pub control: Control,
}

fn toggle(id: Id, label: &str, hint: &str, on: bool) -> Row {
    Row { id: Some(id), label: label.into(), hint: hint.into(), control: Control::Toggle(on) }
}

fn heading(label: &str) -> Row {
    Row { id: None, label: label.into(), hint: String::new(), control: Control::Heading }
}

/// Строки вкладки `tab` по текущему конфигу.
pub fn rows(tab: usize, cfg: &Config) -> Vec<Row> {
    match tab {
        0 => {
            let hidden = baritems::parse_hidden(&cfg.bar_hidden);
            let mut out = vec![heading("Какие значки показывать в панели")];
            out.extend((0..baritems::KINDS.len()).map(|k| toggle(Id::BarItem(k), baritems::title(k), "", !hidden.contains(&k))));
            out
        }
        1 => vec![
            heading("Панель"),
            toggle(Id::BarAutohide, "Скрывать панель автоматически", "Выезжает, когда курсор у верхнего края", cfg.bar_autohide),
            toggle(Id::BarSolid, "Всегда сплошная панель", "Иначе прозрачная над рабочим столом", cfg.bar_solid),
            toggle(Id::BatteryPercent, "Проценты рядом с батареей", "", cfg.battery_percent),
            heading("Часы"),
            toggle(Id::ClockSeconds, "Показывать секунды", "", cfg.clock.seconds),
            toggle(Id::Clock12h, "12-часовой формат", "", cfg.clock.hour12),
            toggle(Id::ClockDate, "Показывать дату", "", !cfg.clock.hide_date),
            toggle(Id::ClockWeekday, "Показывать день недели", "", !cfg.clock.hide_weekday),
        ],
        2 => {
            let selected = DOCK_MODES.iter().position(|m| m.0 == cfg.dock).unwrap_or(0);
            vec![
                heading("Dock"),
                Row {
                    id: Some(Id::DockMode),
                    label: "Показ Dock".into(),
                    hint: "Умный прячется под окнами; «Выключен» оставляет панель задач Windows".into(),
                    control: Control::Cycle(DOCK_MODES.iter().map(|m| m.1.to_string()).collect(), selected),
                },
                heading("Окна"),
                toggle(Id::Stage, "Stage Manager", "Одно приложение на сцене, остальные в полоске слева", cfg.stage_on),
            ]
        }
        3 => {
            let present: Vec<usize> = widgets::entries(&cfg.widgets).iter().map(|s| s.kind).collect();
            let mut out = vec![heading("Виджеты на рабочем столе (размер и цвет: правый клик по виджету)")];
            out.extend((0..widgets::KINDS.len()).map(|k| toggle(Id::Widget(k), widgets::title(k), "", present.contains(&k))));
            out
        }
        4 => {
            let mut out = vec![
                heading("Клавиши"),
                toggle(Id::WinTap, "Win открывает Dock и сетку приложений", "Вместо меню Пуск", !cfg.win_start_keep),
                toggle(Id::Switcher, "Alt+Tab и Win+Tab: переключатель приложений", "Вместо системного", !cfg.switcher_off),
                toggle(Id::SpotlightKey, "Win+Alt открывает Spotlight", "", !cfg.spotlight_off),
                heading("Горячие углы"),
            ];
            for (i, name) in CORNER_NAMES.iter().enumerate() {
                let selected = CORNER_ACTIONS.iter().position(|a| a.0 == cfg.corners[i]).unwrap_or(0);
                out.push(Row {
                    id: Some(Id::Corner(i)),
                    label: (*name).into(),
                    hint: String::new(),
                    control: Control::Cycle(CORNER_ACTIONS.iter().map(|a| a.1.to_string()).collect(), selected),
                });
            }
            out
        }
        _ => {
            let accent = cfg.accent.unwrap_or(ACCENTS[0].1);
            vec![
                heading("Цвет системы: выбранное, кружки, шкалы, кнопки"),
                Row {
                    id: Some(Id::Accent),
                    label: "Цвет системы".into(),
                    hint: ACCENTS.iter().find(|a| a.1 == accent).map_or("Свой", |a| a.0).into(),
                    control: Control::Swatches(ACCENTS.iter().map(|a| a.1).collect(), ACCENTS.iter().position(|a| a.1 == accent)),
                },
                Row {
                    id: Some(Id::WidgetsColor),
                    label: "Цвет виджетов".into(),
                    hint: "Для всех сразу; у каждого свой: правый клик по виджету".into(),
                    control: Control::Swatches(
                        widgets::COLORS.iter().map(|c| [(c.2[0] * 255.0) as u8, (c.2[1] * 255.0) as u8, (c.2[2] * 255.0) as u8]).collect(),
                        {
                            let colors: Vec<usize> = widgets::entries(&cfg.widgets).iter().map(|s| s.color).collect();
                            colors.first().copied().filter(|c| colors.iter().all(|x| x == c))
                        },
                    ),
                },
                Row {
                    id: Some(Id::BarColor),
                    label: "Цвет сплошной панели".into(),
                    hint: BAR_COLORS.iter().find(|a| a.1 == cfg.bar_color).map_or("Свой", |a| a.0).into(),
                    control: Control::Swatches(BAR_COLORS.iter().map(|a| a.1).collect(), BAR_COLORS.iter().position(|a| a.1 == cfg.bar_color)),
                },
                heading("Стеклянные поверхности"),
                Row { id: Some(Id::GlassDensity), label: "Плотность стекла".into(), hint: "Меню и всплывающие панели".into(), control: Control::Slider(cfg.glass_density.unwrap_or(40)) },
            ]
        }
    }
}

/// Применяет значение строки к конфигу: выключатель 0 или 1, выбор по кругу номер варианта, цвет номер цвета.
pub fn apply(cfg: &mut Config, id: Id, value: usize) {
    let on = value == 1;
    match id {
        Id::BarItem(k) => {
            let mut hidden = baritems::parse_hidden(&cfg.bar_hidden);
            hidden.retain(|h| *h != k);
            if !on {
                hidden.push(k);
                hidden.sort_unstable();
            }
            cfg.bar_hidden = baritems::serialize(&hidden);
        }
        Id::BarAutohide => cfg.bar_autohide = on,
        Id::BarSolid => cfg.bar_solid = on,
        Id::BatteryPercent => cfg.battery_percent = on,
        Id::ClockSeconds => cfg.clock.seconds = on,
        Id::Clock12h => cfg.clock.hour12 = on,
        Id::ClockDate => cfg.clock.hide_date = !on,
        Id::ClockWeekday => cfg.clock.hide_weekday = !on,
        Id::DockMode => cfg.dock = DOCK_MODES.get(value).map_or(DockMode::Smart, |m| m.0),
        Id::Stage => cfg.stage_on = on,
        Id::Widget(k) => {
            if on {
                if !widgets::entries(&cfg.widgets).iter().any(|s| s.kind == k) {
                    let (sw, scale) = (unsafe { GetSystemMetrics(SM_CXSCREEN) }, primary_scale());
                    let (x, y) = widgets::default_position(cfg.widgets.len(), k, sw as f32 / scale);
                    widgets::upsert(&mut cfg.widgets, k, x, y);
                }
            } else {
                widgets::remove(&mut cfg.widgets, k);
            }
        }
        Id::WinTap => cfg.win_start_keep = !on,
        Id::Switcher => cfg.switcher_off = !on,
        Id::SpotlightKey => cfg.spotlight_off = !on,
        Id::Corner(i) => cfg.corners[i.min(3)] = CORNER_ACTIONS.get(value).map_or("", |a| a.0).to_string(),
        Id::Accent => cfg.accent = ACCENTS.get(value).map(|a| a.1).filter(|c| *c != ACCENTS[0].1),
        Id::WidgetsColor => {
            let kinds: Vec<usize> = widgets::entries(&cfg.widgets).iter().map(|s| s.kind).collect();
            for k in kinds {
                widgets::set_color(&mut cfg.widgets, k, value);
            }
        }
        Id::BarColor => cfg.bar_color = BAR_COLORS.get(value).map_or([0, 0, 0], |a| a.1),
        Id::GlassDensity => cfg.glass_density = Some(value.clamp(10, 85) as u8),
    }
}

fn to_slint(r: &Row) -> SetRow {
    let strings = |v: &[String]| ModelRc::new(VecModel::from(v.iter().map(|s| SharedString::from(s.as_str())).collect::<Vec<_>>()));
    let none = ModelRc::new(VecModel::<SharedString>::default());
    let no_colors = ModelRc::new(VecModel::<Color>::default());
    let base = SetRow { label: r.label.as_str().into(), hint: r.hint.as_str().into(), kind: 3, value: 0, options: none.clone(), colors: no_colors.clone() };
    match &r.control {
        Control::Heading => base,
        Control::Toggle(on) => SetRow { kind: 0, value: i32::from(*on), ..base },
        Control::Cycle(options, selected) => SetRow { kind: 1, value: *selected as i32, options: strings(options), ..base },
        Control::Slider(value) => SetRow { kind: 4, value: *value as i32, ..base },
        Control::Swatches(colors, selected) => SetRow {
            kind: 2,
            value: selected.map_or(-1, |s| s as i32),
            colors: ModelRc::new(VecModel::from(colors.iter().map(|c| Color::from_rgb_u8(c[0], c[1], c[2])).collect::<Vec<_>>())),
            ..base
        },
    }
}

/// Тот же материал поверх участка текущих обоев, без захвата других окон.
pub(crate) fn glass_preview(density: u8) -> Option<slint::Image> {
    use crate::bar_style::{crop, soft_blur};
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if sw < 480 || sh < 96 { return None; }
    let wall = crate::wallpaper::screen(sw as u32, sh as u32)?;
    let raw = crop(&wall, ((sw - 480) / 2) as u32, ((sh - 96) / 2) as u32, 480, 96);
    let blurred = soft_blur(&raw, 12);
    let mut params = popup_glass_params();
    params.blur_radius = 0;
    params.tint[3] = f32::from(density.clamp(10, 85)) / 100.0;
    let glass = cpu::render(&blurred, &params);
    let mut preview = raw;
    cpu::composite_over(&mut preview, &glass.pixels);
    let rgba: Vec<u8> = preview.pixels.iter().flat_map(|p| [((p >> 16) & 255) as u8, ((p >> 8) & 255) as u8, (p & 255) as u8, 255]).collect();
    Some(slint::Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, 480, 96)))
}

pub struct SettingsWin {
    win: LayeredWindow,
    view: ContentView<Settings>,
    glass: Bitmap,
    tab: Cell<usize>,
    rows: RefCell<Vec<Row>>,
    changed: Rc<Cell<Option<(i32, i32)>>>,
    tab_picked: Rc<Cell<i32>>,
    close: Rc<Cell<bool>>,
    dirty: Cell<bool>,
    previous: isize,
}

impl SettingsWin {
    pub fn open(cfg: &Config) -> Result<SettingsWin> {
        let scale = primary_scale();
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let (w, h) = ((WIDTH_PT * scale).round() as i32, (HEIGHT_PT * scale).round() as i32);
        let previous = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize };
        let mut win = LayeredWindow::new_focusable(RectI { x: (sw - w) / 2, y: (sh - h) / 2, w, h })?;
        win.animate_overlay(true, (8.0 * scale).round() as i32);
        let glass = popup_glass(win.rect(), &popup_glass_params().scaled(scale))?;
        let view = ContentView::new_with(w as u32, h as u32, Settings::new);
        view.set_scale_factor(scale);
        view.ui.set_tabs(ModelRc::new(VecModel::from(TABS.iter().map(|t| SharedString::from(*t)).collect::<Vec<_>>())));
        view.ui.set_note("Изменения применяются, когда окно закрыто: macshell перезапустится сам.".into());
        let changed = Rc::new(Cell::new(None));
        let slot = changed.clone();
        view.ui.on_changed(move |row, value| slot.set(Some((row, value))));
        let tab_picked = Rc::new(Cell::new(-1));
        let slot = tab_picked.clone();
        view.ui.on_tab_selected(move |i| slot.set(i));
        let close = Rc::new(Cell::new(false));
        let slot = close.clone();
        view.ui.on_dismiss(move || slot.set(true));
        let this = SettingsWin { win, view, glass, tab: Cell::new(0), rows: RefCell::new(Vec::new()), changed, tab_picked, close, dirty: Cell::new(false), previous };
        this.show(cfg);
        this.present(true)?;
        this.win.focus();
        Ok(this)
    }

    fn show(&self, cfg: &Config) {
        let rows = rows(self.tab.get(), cfg);
        self.view.ui.set_tab(self.tab.get() as i32);
        self.view.ui.set_rows(ModelRc::new(VecModel::from(rows.iter().map(to_slint).collect::<Vec<_>>())));
        if self.tab.get() == 5 {
            if let Some(image) = glass_preview(cfg.glass_density.unwrap_or(40)) {
                self.view.ui.set_glass_preview(image);
                self.view.ui.set_preview_available(true);
            }
        }
        *self.rows.borrow_mut() = rows;
    }

    /// Клавиатура в собственном оконном хосте: Slint получает мышь, а клавиши обрабатываются здесь.
    fn key(&self, vk: u32, shift: bool) -> bool {
        let choice = self.view.ui.get_choice_row();
        if choice >= 0 {
            let rows = self.rows.borrow();
            let Some(row) = rows.get(choice as usize) else { return false };
            let Control::Cycle(options, _) = &row.control else { return false };
            match vk {
                0x1B => self.view.ui.set_choice_row(-1),
                0x26 | 0x28 => {
                    let delta = if vk == 0x26 { -1 } else { 1 };
                    let next = (self.view.ui.get_choice_index() + delta).rem_euclid(options.len() as i32);
                    self.view.ui.set_choice_index(next);
                }
                0x0D | 0x20 => {
                    self.changed.set(Some((choice, self.view.ui.get_choice_index())));
                    self.view.ui.set_choice_row(-1);
                }
                _ => {}
            }
            return false;
        }
        if vk == VK_ESCAPE { return true; }
        if vk == 0x21 || vk == 0x22 {
            let delta = if vk == 0x21 { -1 } else { 1 };
            self.tab_picked.set((self.tab.get() as i32 + delta).rem_euclid(TABS.len() as i32));
            return false;
        }
        let rows = self.rows.borrow();
        if matches!(vk, 0x09 | 0x26 | 0x28) {
            let backwards = vk == 0x26 || (vk == 0x09 && shift);
            let len = rows.len() as i32;
            if len > 0 {
                let current = self.view.ui.get_keyboard_row();
                let mut index = if current < 0 { if backwards { 0 } else { -1 } } else { current };
                for _ in 0..len {
                    index = (index + if backwards { -1 } else { 1 }).rem_euclid(len);
                    if rows[index as usize].id.is_some() { break; }
                }
                self.view.ui.set_keyboard_row(index);
            }
        } else if matches!(vk, 0x25 | 0x27) {
            let index = self.view.ui.get_keyboard_row();
            if let Some(Row { control: Control::Slider(value), .. }) = rows.get(index.max(0) as usize).filter(|_| index >= 0) {
                self.changed.set(Some((index, (i32::from(*value) + if vk == 0x25 { -5 } else { 5 }).clamp(10, 85))));
            }
        } else if matches!(vk, 0x0D | 0x20) {
            let index = self.view.ui.get_keyboard_row();
            if let Some(row) = rows.get(index.max(0) as usize).filter(|_| index >= 0) {
                match &row.control {
                    Control::Toggle(on) => self.changed.set(Some((index, i32::from(!on)))),
                    Control::Cycle(_, selected) => {
                        self.view.ui.set_choice_index(*selected as i32);
                        self.view.ui.set_choice_row(index);
                    }
                    _ => {}
                }
            }
        }
        false
    }

    fn present(&self, force: bool) -> Result<()> {
        let content = match self.view.render_if_changed() {
            Some(c) => c,
            None if force => self.view.render(),
            None => return Ok(()),
        };
        let mut out = self.glass.clone();
        cpu::composite_over(&mut out, &content);
        self.win.present(&out)
    }

    /// Один шаг: щелчки и клавиши, запись в конфиг. Возвращает `(закрыть окно, что-то менялось)`.
    pub fn tick(&self, cfg: &mut Config) -> Result<(bool, bool)> {
        let mut close = false;
        for ev in self.win.take_input() {
            match ev {
                InputEvent::Key { vk, shift, .. } => close |= self.key(vk, shift),
                InputEvent::Char(' ') => close |= self.key(0x20, false),
                other => self.view.dispatch(&other),
            }
        }
        let tab = self.tab_picked.replace(-1);
        if tab >= 0 {
            self.tab.set((tab as usize).min(TABS.len() - 1));
            self.view.ui.set_choice_row(-1);
            self.view.ui.set_keyboard_row(-1);
            self.show(cfg);
        }
        if let Some((row, value)) = self.changed.take() {
            let id = self.rows.borrow().get(row as usize).and_then(|r| r.id);
            if let Some(id) = id {
                apply(cfg, id, value.max(0) as usize);
                if id == Id::GlassDensity { crate::bar::set_glass_density(cfg.glass_density.unwrap_or(40)); }
                let _ = cfg.save();
                self.dirty.set(true);
                if id == Id::GlassDensity {
                    let updated = rows(self.tab.get(), cfg);
                    if let Some(changed) = updated.get(row as usize) {
                        self.view.ui.get_rows().set_row_data(row as usize, to_slint(changed));
                    }
                    *self.rows.borrow_mut() = updated;
                    if let Some(image) = glass_preview(cfg.glass_density.unwrap_or(40)) {
                        self.view.ui.set_glass_preview(image);
                        self.view.ui.set_preview_available(true);
                    }
                } else {
                    self.show(cfg);
                }
            }
        }
        self.present(false)?;
        Ok((close || self.close.get(), self.dirty.get()))
    }

    pub fn animating(&self) -> bool {
        self.view.animating()
    }
}

impl Drop for SettingsWin {
    fn drop(&mut self) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};
        let previous = HWND(self.previous as *mut _);
        if self.win.is_foreground() && unsafe { IsWindow(Some(previous)) }.as_bool() {
            unsafe { let _ = SetForegroundWindow(previous); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_choice_opens_list_and_selects_a_specific_action() {
        let view = ContentView::new_with(720, 500, Settings::new);
        view.ui.set_tabs(ModelRc::new(VecModel::from(TABS.iter().map(|t| SharedString::from(*t)).collect::<Vec<_>>())));
        view.ui.set_tab(4);
        view.ui.set_rows(ModelRc::new(VecModel::from(rows(4, &Config::default()).iter().map(to_slint).collect::<Vec<_>>())));
        let chosen = Rc::new(Cell::new(None));
        let result = chosen.clone();
        view.ui.on_changed(move |row, value| result.set(Some((row, value))));
        view.render();
        let click = |x, y| {
            for ev in [InputEvent::Move { x, y }, InputEvent::Down { x, y }, InputEvent::Up { x, y }] { view.dispatch(&ev); }
        };
        click(550, 275);
        assert_eq!(view.ui.get_choice_row(), 5);
        view.render();
        click(550, 182);
        assert_eq!(chosen.get(), Some((5, 2)));
        assert_eq!(view.ui.get_choice_row(), -1);
    }

    #[test]
    fn glass_slider_reports_a_bounded_value() {
        let view = ContentView::new_with(720, 500, Settings::new);
        view.ui.set_tabs(ModelRc::new(VecModel::from(TABS.iter().map(|t| SharedString::from(*t)).collect::<Vec<_>>())));
        view.ui.set_tab(5);
        view.ui.set_rows(ModelRc::new(VecModel::from(rows(5, &Config::default()).iter().map(to_slint).collect::<Vec<_>>())));
        let chosen = Rc::new(Cell::new(None));
        let out = chosen.clone();
        view.ui.on_changed(move |row, value| out.set(Some((row, value))));
        view.render();
        for ev in [InputEvent::Move { x: 570, y: 275 }, InputEvent::Down { x: 570, y: 275 }, InputEvent::Up { x: 570, y: 275 }] { view.dispatch(&ev); }
        let (row, value) = chosen.get().expect("slider click");
        assert_eq!(row, 5);
        assert!((40..=55).contains(&value), "slider returned {value}");
    }

    #[test]
    fn glass_slider_keeps_drag_after_value_refresh() {
        use slint::Model;
        let view = ContentView::new_with(720, 500, Settings::new);
        view.ui.set_tabs(ModelRc::new(VecModel::from(TABS.iter().map(|t| SharedString::from(*t)).collect::<Vec<_>>())));
        view.ui.set_tab(5);
        let mut cfg = Config::default();
        view.ui.set_rows(ModelRc::new(VecModel::from(rows(5, &cfg).iter().map(to_slint).collect::<Vec<_>>())));
        let chosen = Rc::new(Cell::new(None));
        let out = chosen.clone();
        view.ui.on_changed(move |row, value| out.set(Some((row, value))));
        view.render();
        view.dispatch(&InputEvent::Move { x: 510, y: 275 });
        view.dispatch(&InputEvent::Down { x: 510, y: 275 });
        let (_, first) = chosen.take().unwrap();
        apply(&mut cfg, Id::GlassDensity, first as usize);
        view.ui.get_rows().set_row_data(5, to_slint(&rows(5, &cfg)[5]));
        view.render();
        view.dispatch(&InputEvent::Move { x: 640, y: 275 });
        assert!(chosen.get().is_some_and(|(_, second)| second > first + 30), "drag must continue after preview refresh");
    }

    fn toggles(tab: usize, cfg: &Config) -> Vec<(Option<Id>, bool)> {
        rows(tab, cfg).into_iter().filter_map(|r| if let Control::Toggle(on) = r.control { Some((r.id, on)) } else { None }).collect()
    }

    #[test]
    fn every_tab_has_rows_and_every_row_with_a_control_has_an_id() {
        let cfg = Config::default();
        for tab in 0..TABS.len() {
            let list = rows(tab, &cfg);
            assert!(!list.is_empty() && list.len() <= 11, "tab {tab}: {} rows do not fit", list.len());
            assert!(list.iter().all(|r| r.id.is_some() == !matches!(r.control, Control::Heading)), "tab {tab}");
        }
    }

    #[test]
    fn bar_icon_toggles_follow_and_write_the_hidden_list() {
        let mut cfg = Config::default();
        assert!(toggles(0, &cfg).iter().all(|t| t.1), "all icons shown by default");
        apply(&mut cfg, Id::BarItem(3), 0);
        apply(&mut cfg, Id::BarItem(1), 0);
        assert_eq!(cfg.bar_hidden, "focus,vpn");
        assert!(!toggles(0, &cfg).iter().find(|t| t.0 == Some(Id::BarItem(3))).unwrap().1);
        apply(&mut cfg, Id::BarItem(3), 1);
        assert_eq!(cfg.bar_hidden, "focus");
    }

    #[test]
    fn key_toggles_are_inverted_flags() {
        let mut cfg = Config::default();
        let on = |cfg: &Config, id| toggles(4, cfg).into_iter().find(|t| t.0 == Some(id)).unwrap().1;
        assert!(on(&cfg, Id::WinTap) && on(&cfg, Id::Switcher) && on(&cfg, Id::SpotlightKey));
        apply(&mut cfg, Id::WinTap, 0);
        apply(&mut cfg, Id::Switcher, 0);
        assert!(cfg.win_start_keep && cfg.switcher_off && !cfg.spotlight_off);
        assert!(!on(&cfg, Id::WinTap) && !on(&cfg, Id::Switcher));
    }

    #[test]
    fn clock_toggles_write_the_clock_options() {
        let mut cfg = Config::default();
        apply(&mut cfg, Id::ClockSeconds, 1);
        apply(&mut cfg, Id::ClockDate, 0);
        assert!(cfg.clock.seconds && cfg.clock.hide_date && !cfg.clock.hide_weekday);
    }

    #[test]
    fn cycle_rows_pick_dock_mode_and_corner_actions() {
        let mut cfg = Config::default();
        apply(&mut cfg, Id::DockMode, 2);
        assert_eq!(cfg.dock, DockMode::Fixed);
        apply(&mut cfg, Id::Corner(1), 1);
        apply(&mut cfg, Id::Corner(3), 4);
        assert_eq!(cfg.corners, [String::new(), "mission".to_string(), String::new(), "desktop".to_string()]);
        let corner = rows(4, &cfg).into_iter().find(|r| r.id == Some(Id::Corner(3))).unwrap();
        assert_eq!(corner.control, Control::Cycle(CORNER_ACTIONS.iter().map(|a| a.1.to_string()).collect(), 4));
    }

    #[test]
    fn colors_are_stored_and_default_blue_is_not_written() {
        let mut cfg = Config::default();
        apply(&mut cfg, Id::Accent, 2);
        assert_eq!(cfg.accent, Some(ACCENTS[2].1));
        apply(&mut cfg, Id::Accent, 0);
        assert_eq!(cfg.accent, None, "the default blue leaves the config clean");
        apply(&mut cfg, Id::BarColor, 3);
        assert_eq!(cfg.bar_color, BAR_COLORS[3].1);
        let swatch = rows(5, &cfg).into_iter().find(|r| r.id == Some(Id::BarColor)).unwrap();
        assert!(matches!(swatch.control, Control::Swatches(_, Some(3))));
    }

    #[test]
    fn the_widgets_color_row_paints_every_widget() {
        let mut cfg = Config::default();
        apply(&mut cfg, Id::Widget(0), 1);
        apply(&mut cfg, Id::Widget(2), 1);
        apply(&mut cfg, Id::WidgetsColor, 1);
        assert!(widgets::entries(&cfg.widgets).iter().all(|s| s.color == 1));
        let row = rows(5, &cfg).into_iter().find(|r| r.id == Some(Id::WidgetsColor)).unwrap();
        assert!(matches!(row.control, Control::Swatches(_, Some(1))));
        widgets::set_color(&mut cfg.widgets, 0, 3);
        let row = rows(5, &cfg).into_iter().find(|r| r.id == Some(Id::WidgetsColor)).unwrap();
        assert!(matches!(row.control, Control::Swatches(_, None)), "mixed colors select nothing");
    }

    #[test]
    fn widget_toggles_add_and_remove_lines() {
        let mut cfg = Config::default();
        apply(&mut cfg, Id::Widget(7), 1);
        apply(&mut cfg, Id::Widget(7), 1);
        assert_eq!(widgets::entries(&cfg.widgets).len(), 1, "no duplicates");
        assert!(toggles(3, &cfg).iter().any(|t| t.0 == Some(Id::Widget(7)) && t.1));
        apply(&mut cfg, Id::Widget(7), 0);
        assert!(cfg.widgets.is_empty());
    }

    #[test]
    #[ignore = "opens a real window on the desktop"]
    fn window_opens_and_switches_tabs() {
        let mut cfg = Config::default();
        let win = SettingsWin::open(&cfg).expect("open");
        win.tab_picked.set(4);
        let (close, dirty) = win.tick(&mut cfg).expect("tick");
        assert!(!close && !dirty);
        assert_eq!(win.tab.get(), 4);
    }
}
