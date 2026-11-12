//! Виджеты на рабочем столе: часы, календарь, нагрузка системы. Каждый виджет это маленькое окно со скруглённой
//! тёмной плитой, лежащее над обоями и под окнами приложений (владелец окна `Progman`, поэтому «Показать рабочий
//! стол» их не прячет). Окно двигается мышью в любом месте, положение запоминается с шагом сетки в 8 пикселей.
//! Время обновляется раз в минуту, нагрузка раз в пять секунд, и только пока виджет есть на столе.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use capture::RectI;
use glass::{Bitmap, GlassParams, cpu};
use slint::{ModelRc, VecModel};
use windows::Win32::Foundation::{FILETIME, HWND, POINT};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GWLP_HWNDPARENT, GetCursorPos, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos,
};
use windows::core::w;

use crate::bar::menu_glass;
use crate::bar_style::{crop, padded_rect, soft_blur};
use crate::calendar;
use crate::clock::LocalTime;
use crate::host::{InputEvent, LayeredWindow, primary_scale};
use crate::slint_view::{
    CalDay, CalendarPopup, ClockWidget, ContentView, CurrencyWidget, DriveInfo, NotesWidget, NowPlayingWidget, RateRow, StorageWidget, SystemWidget, WeatherWidget, WidgetGuide,
};
use crate::widgetdata::{self, Weather};
use crate::{currency, media};
use std::sync::mpsc::{Receiver, TryRecvError};

pub const KINDS: [&str; 8] = ["clock", "calendar", "system", "now", "storage", "weather", "currency", "notes"];
/// Шаг сетки привязки, логические пиксели.
const GRID: i32 = 8;
const RADIUS_PT: f32 = 22.0;
const SYSTEM_TICK: Duration = Duration::from_secs(5);
/// Сдвиг мыши, с которого нажатие считается перетаскиванием, а не кликом.
const DRAG_PX: i32 = 4;

pub fn title(kind: usize) -> &'static str {
    ["Часы", "Календарь", "Нагрузка системы", "Сейчас играет", "Диски", "Погода", "Курс валют", "Заметки"].get(kind).copied().unwrap_or("")
}

/// Размер окна виджета, логические пиксели.
pub fn size_pt(kind: usize) -> (f32, f32) {
    match kind {
        1 => (280.0, 320.0),
        0 | 6 => (220.0, 152.0),
        _ => (220.0, 150.0),
    }
}

pub fn kind_of(name: &str) -> Option<usize> {
    KINDS.iter().position(|k| k.eq_ignore_ascii_case(name.trim()))
}

/// Размеры виджета: подпись, имя в конфиге и множитель к основному размеру.
pub const SIZES: [(&str, &str, f32); 3] = [("Маленький", "s", 0.75), ("Средний", "m", 1.0), ("Большой", "l", 1.4)];

/// Цвета плиты: подпись, имя в конфиге, цвет подкраски стекла. Первый (обычный) тёмный, как всегда.
pub const COLORS: [(&str, &str, [f32; 3]); 8] = [
    ("Обычный", "", [0.05, 0.06, 0.09]),
    ("Синий", "blue", [0.10, 0.22, 0.58]),
    ("Фиолетовый", "purple", [0.36, 0.16, 0.58]),
    ("Розовый", "pink", [0.62, 0.16, 0.38]),
    ("Красный", "red", [0.58, 0.12, 0.12]),
    ("Оранжевый", "orange", [0.64, 0.34, 0.08]),
    ("Зелёный", "green", [0.10, 0.42, 0.24]),
    ("Графит", "graphite", [0.17, 0.17, 0.19]),
];

/// Один виджет из конфига: вид, место в логических пикселях, размер и цвет (номера в `SIZES` и `COLORS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub kind: usize,
    pub x: i32,
    pub y: i32,
    pub size: usize,
    pub color: usize,
}

impl Spec {
    pub fn new(kind: usize, x: i32, y: i32) -> Spec {
        Spec { kind, x, y, size: 1, color: 0 }
    }

    /// Новое место; размер и цвет берутся у уже сохранённого виджета этого вида, если он есть.
    pub fn saved(list: &[String], kind: usize, x: i32, y: i32) -> Spec {
        let old = entries(list).into_iter().find(|s| s.kind == kind);
        Spec { x, y, size: old.map_or(1, |o| o.size), color: old.map_or(0, |o| o.color), ..Spec::new(kind, x, y) }
    }

    /// Размер окна в логических пикселях с учётом выбранного размера.
    pub fn window_pt(&self) -> (f32, f32) {
        let (w, h) = size_pt(self.kind);
        let k = SIZES[self.size.min(2)].2;
        ((w * k).round(), (h * k).round())
    }
}

/// «clock,40,64» или «clock,40,64,l,blue»: вид, место, размер и цвет (последние два можно опустить).
/// Ошибочные строки пропускаются.
pub fn parse(text: &str) -> Option<Spec> {
    let mut parts = text.split(',').map(str::trim);
    let kind = kind_of(parts.next()?)?;
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    let size = parts.next().and_then(|n| SIZES.iter().position(|s| s.1.eq_ignore_ascii_case(n))).unwrap_or(1);
    let color = parts.next().and_then(|n| COLORS.iter().position(|c| !c.1.is_empty() && c.1.eq_ignore_ascii_case(n))).unwrap_or(0);
    Some(Spec { kind, x, y, size, color })
}

pub fn serialize(spec: &Spec) -> String {
    let mut line = format!("{},{},{}", KINDS[spec.kind], spec.x, spec.y);
    if spec.size != 1 || spec.color != 0 {
        line += &format!(",{}", SIZES[spec.size.min(2)].1);
    }
    if spec.color != 0 {
        line += &format!(",{}", COLORS[spec.color.min(COLORS.len() - 1)].1);
    }
    line
}

/// Значение, прижатое к сетке.
pub fn snap(v: i32) -> i32 {
    ((v as f32 / GRID as f32).round() as i32) * GRID
}

/// Ближайшие кромки и промежуток 16 pt; далёкие по другой оси виджеты не притягивают.
pub fn snap_to_neighbors(moving: Spec, neighbors: &[Spec]) -> (i32, i32, String, bool, bool) {
    let (w, h) = moving.window_pt();
    let (w, h) = (w as i32, h as i32);
    let mut best_x: Option<(i32, i32, bool)> = None;
    let mut best_y: Option<(i32, i32, bool)> = None;
    let consider = |best: &mut Option<(i32, i32, bool)>, at: i32, target: i32, gap: bool| {
        let distance = (at - target).abs();
        if distance <= 6 && best.is_none_or(|b| distance < b.0) { *best = Some((distance, target, gap)); }
    };
    for n in neighbors.iter().filter(|n| n.kind != moving.kind) {
        let (nw, nh) = n.window_pt();
        let (nw, nh) = (nw as i32, nh as i32);
        if moving.y <= n.y + nh + 32 && moving.y + h + 32 >= n.y {
            for target in [n.x, n.x + nw - w] { consider(&mut best_x, moving.x, target, false); }
            for target in [n.x + nw + 16, n.x - w - 16] { consider(&mut best_x, moving.x, target, true); }
        }
        if moving.x <= n.x + nw + 32 && moving.x + w + 32 >= n.x {
            for target in [n.y, n.y + nh - h] { consider(&mut best_y, moving.y, target, false); }
            for target in [n.y + nh + 16, n.y - h - 16] { consider(&mut best_y, moving.y, target, true); }
        }
    }
    let aligned = best_x.is_some_and(|x| !x.2) || best_y.is_some_and(|y| !y.2);
    let gap = best_x.is_some_and(|x| x.2) || best_y.is_some_and(|y| y.2);
    let label = match (aligned, gap) { (true, true) => "Общий край · отступ 16", (true, false) => "Общий край", (false, true) => "Отступ 16", _ => "" };
    (best_x.map_or(moving.x, |x| x.1), best_y.map_or(moving.y, |y| y.1), label.to_string(), best_x.is_some(), best_y.is_some())
}

/// Куда ставить новый виджет: правый верхний угол, каждый следующий ниже и левее, чтобы не лечь поверх.
pub fn default_position(existing: usize, kind: usize, screen_w_pt: f32) -> (i32, i32) {
    let (w, _) = size_pt(kind);
    let x = (screen_w_pt - w - 32.0 - existing as f32 * 24.0).max(0.0) as i32;
    (snap(x), snap(48 + existing as i32 * 160))
}

/// Доля покрытия пикселя скруглённым прямоугольником `w` на `h` с радиусом `r`: 1 внутри, 0 снаружи, плавно на краю.
pub fn coverage(x: u32, y: u32, w: u32, h: u32, r: f32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let (cx, cy) = (px.clamp(r, w as f32 - r), py.clamp(r, h as f32 - r));
    let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    (r - dist + 0.5).clamp(0.0, 1.0)
}

/// Плита виджета: тёмная, чуть прозрачная, с тонкой светлой кромкой. Пиксели предумножены (формат `UpdateLayeredWindow`).
pub fn panel(w: u32, h: u32, radius: f32) -> Bitmap {
    let mut b = Bitmap::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = coverage(x, y, w, h, radius);
            if c <= 0.0 {
                continue;
            }
            // Кромка: внешняя полоска шириной 1 px светлее и плотнее середины.
            let inner = x > 0 && y > 0 && x + 1 < w && y + 1 < h && coverage(x - 1, y - 1, w, h, radius - 1.0) >= 1.0;
            let (r, g, bl, a) = if inner { (24.0, 26.0, 34.0, 0.66) } else { (90.0, 94.0, 110.0, 0.7) };
            let alpha = a * c;
            let px = |v: f32| (v * alpha).round() as u32;
            b.pixels[(y * w + x) as usize] = (((alpha * 255.0).round() as u32) << 24) | (px(r) << 16) | (px(g) << 8) | px(bl);
        }
    }
    b
}

/// Стекло виджета: размытые обои под окном, как у всплывающих окон панели. Обои берутся из файла, а не со
/// снимка экрана, чтобы окна приложений поверх рабочего стола не попадали в стекло. `None`, если обоев нет или
/// окно заходит за край экрана; тогда остаётся прежняя плита.
pub fn glass_params(scale: f32) -> GlassParams {
    GlassParams {
        radius: RADIUS_PT,
        bevel: 12.0,
        refraction: 6.0,
        blur_radius: 16,
        saturation: 1.45,
        tint: [0.05, 0.06, 0.09, 0.36],
        ..menu_glass()
    }
    .scaled(scale)
}

fn glass_plate(rect: RectI, scale: f32, color: usize) -> Option<Bitmap> {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if rect.x < 0 || rect.y < 0 || rect.x + rect.w > sw || rect.y + rect.h > sh {
        return None;
    }
    let wall = crate::wallpaper::screen(sw as u32, sh as u32)?;
    let mut params = glass_params(scale);
    if color > 0 {
        let c = COLORS[color.min(COLORS.len() - 1)].2;
        params.tint = [c[0], c[1], c[2], 0.5];
    }
    let padded = padded_rect(rect, 2 * params.blur_radius as i32, sw, sh);
    let soft = soft_blur(&crop(&wall, padded.x as u32, padded.y as u32, padded.w as u32, padded.h as u32), params.blur_radius);
    let inner = crop(&soft, (rect.x - padded.x) as u32, (rect.y - padded.y) as u32, rect.w as u32, rect.h as u32);
    Some(cpu::render(&inner, &GlassParams { blur_radius: 0, ..params }))
}

/// Список из конфига; ошибочные строки и повторы вида пропускаются.
pub fn entries(list: &[String]) -> Vec<Spec> {
    let mut out: Vec<Spec> = Vec::new();
    for e in list.iter().filter_map(|l| parse(l)) {
        if !out.iter().any(|o| o.kind == e.kind) {
            out.push(e);
        }
    }
    out
}

/// Подгоняет прежний блок из аудита под новые размеры, не затрагивая произвольные раскладки.
pub fn align_legacy_triplet(list: &mut [String]) -> bool {
    let all = entries(list);
    let (Some(cal), Some(clock), Some(currency)) = (
        all.iter().find(|s| s.kind == 1),
        all.iter().find(|s| s.kind == 0),
        all.iter().find(|s| s.kind == 6),
    ) else { return false };
    if [cal.size, clock.size, currency.size] != [1, 1, 1]
        || clock.x != cal.x + 296 || clock.y != cal.y
        || currency.x != clock.x || currency.y != clock.y + 160
    {
        return false;
    }
    if let Some(line) = list.iter_mut().find(|line| parse(line).is_some_and(|s| s.kind == 6)) {
        *line = serialize(&Spec { y: clock.y + 168, ..*currency });
        return true;
    }
    false
}

/// Записывает положение виджета: заменяет строку этого вида, размер и цвет сохраняются.
pub fn upsert(list: &mut Vec<String>, kind: usize, x: i32, y: i32) {
    let spec = Spec::saved(list, kind, x, y);
    remove(list, kind);
    list.push(serialize(&spec));
}

/// Меняет размер виджета этого вида (номер в `SIZES`).
pub fn set_size(list: &mut Vec<String>, kind: usize, size: usize) {
    if let Some(old) = entries(list).into_iter().find(|s| s.kind == kind) {
        remove(list, kind);
        list.push(serialize(&Spec { size: size.min(2), ..old }));
    }
}

/// Меняет цвет виджета этого вида (номер в `COLORS`).
pub fn set_color(list: &mut Vec<String>, kind: usize, color: usize) {
    if let Some(old) = entries(list).into_iter().find(|s| s.kind == kind) {
        remove(list, kind);
        list.push(serialize(&Spec { color: color.min(COLORS.len() - 1), ..old }));
    }
}

/// Убирает виджет этого вида из списка.
pub fn remove(list: &mut Vec<String>, kind: usize) {
    list.retain(|l| parse(l).map(|p| p.kind) != Some(kind));
}

/// Файл заметок рядом с конфигом.
fn notes_path() -> std::path::PathBuf {
    crate::config::Config::path().map_or_else(|| std::path::PathBuf::from("notes.txt"), |p| p.with_file_name("notes.txt"))
}

fn wide_progman() -> HWND {
    unsafe { FindWindowW(w!("Progman"), None) }.unwrap_or_default()
}

/// Доля занятости процессора за интервал по двум снимкам `GetSystemTimes` (простой, ядро с простоем, пользователь).
pub fn cpu_load(before: (u64, u64, u64), after: (u64, u64, u64)) -> f32 {
    let idle = after.0.saturating_sub(before.0) as f32;
    let total = (after.1.saturating_sub(before.1) + after.2.saturating_sub(before.2)) as f32;
    if total <= 0.0 { 0.0 } else { (1.0 - idle / total).clamp(0.0, 1.0) }
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn system_times() -> (u64, u64, u64) {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user));
    }
    (ft(idle), ft(kernel), ft(user))
}

/// Занятая память: доля и подпись «6,1 из 15,9 ГБ».
pub fn memory_use() -> (f32, String) {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut m) }.is_err() || m.ullTotalPhys == 0 {
        return (0.0, String::new());
    }
    let gb = |b: u64| format!("{:.1}", b as f64 / (1u64 << 30) as f64).replace('.', ",");
    let used = m.ullTotalPhys - m.ullAvailPhys;
    (used as f32 / m.ullTotalPhys as f32, format!("{} из {} ГБ", gb(used), gb(m.ullTotalPhys)))
}

enum Content {
    Clock(ContentView<ClockWidget>),
    Calendar { view: ContentView<CalendarPopup>, step: Rc<Cell<i32>>, today: Rc<Cell<bool>>, shown: Cell<(i32, u32)> },
    System { view: ContentView<SystemWidget>, last: Cell<(u64, u64, u64)> },
    Now { view: ContentView<NowPlayingWidget>, command: Rc<Cell<Option<media::Command>>>, pending: RefCell<Option<Receiver<Option<media::Now>>>> },
    Storage(ContentView<StorageWidget>),
    Weather { view: ContentView<WeatherWidget>, pending: RefCell<Option<Receiver<Option<Weather>>>>, place: String, loaded: Cell<bool> },
    Currency(ContentView<CurrencyWidget>),
    Notes { view: ContentView<NotesWidget>, text: RefCell<String>, editing: Cell<bool>, since: Cell<Instant> },
}

/// Одно действие над видом любого виджета: у каждого свой тип окна, а вызов один.
macro_rules! on_view {
    ($content:expr, $v:ident => $body:expr) => {
        match $content {
            Content::Clock($v) => $body,
            Content::Calendar { view: $v, .. } => $body,
            Content::System { view: $v, .. } => $body,
            Content::Now { view: $v, .. } => $body,
            Content::Storage($v) => $body,
            Content::Weather { view: $v, .. } => $body,
            Content::Currency($v) => $body,
            Content::Notes { view: $v, .. } => $body,
        }
    };
}

/// Как часто виджет перечитывает данные, если сам не просил иначе.
fn interval(kind: usize) -> Duration {
    match kind {
        2 => SYSTEM_TICK,
        3 => Duration::from_secs(4),
        4 => Duration::from_secs(60),
        5 => Duration::from_secs(60),
        _ => Duration::from_secs(30),
    }
}

pub struct WidgetWin {
    pub kind: usize,
    win: LayeredWindow,
    panel: Bitmap,
    content: Content,
    scale: f32,
    /// Нажатие: где в окне взяли (физические пиксели) и где курсор был (экран); `moved`: уже тащим.
    press: Option<(i32, i32, i32, i32)>,
    moved: bool,
    next_refresh: Instant,
    last_minute: Cell<u16>,
    /// Через сколько в следующий раз перечитать данные, если виджет просит не как обычно (ждёт ответа сети).
    retry_in: Cell<Option<Duration>>,
    spec: Spec,
    /// Правый клик: где (экран, физические пиксели); бар забирает и открывает меню виджета.
    context: Cell<Option<(i32, i32)>>,
    calendar_focused: Cell<bool>,
    calendar_previous: Cell<Option<isize>>,
    guide: Option<ContentView<WidgetGuide>>,
    guide_axes: (bool, bool),
    guide_visible: bool,
}

impl WidgetWin {
    /// Окно виджета с левым верхним углом в `(x, y)` логических пикселей.
    /// `place`: место для погоды из конфига (`weather = "…"`), пусто, если не задано.
    pub fn open(spec: Spec, place: &str) -> Result<WidgetWin> {
        let (kind, x_pt, y_pt) = (spec.kind, spec.x, spec.y);
        let scale = primary_scale();
        let k = SIZES[spec.size.min(2)].2;
        let (w_pt, h_pt) = spec.window_pt();
        let (w, h) = ((w_pt * scale).round() as i32, (h_pt * scale).round() as i32);
        let rect = RectI { x: (x_pt as f32 * scale).round() as i32, y: (y_pt as f32 * scale).round() as i32, w, h };
        let mut win = if kind == 1 || kind == 7 { LayeredWindow::new_focusable(rect)? } else { LayeredWindow::new(rect)? };
        win.animate_overlay(true, (5.0 * scale).round() as i32);
        win.set_title("macshell.widget");
        // Окно принадлежит рабочему столу: над обоями, под окнами приложений, переживает «Показать рабочий стол».
        unsafe {
            SetWindowLongPtrW(HWND(win.hwnd() as *mut _), GWLP_HWNDPARENT, wide_progman().0 as isize);
            let _ = SetWindowPos(HWND(win.hwnd() as *mut _), Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        let content = match kind {
            0 => {
                let view = ContentView::new_with(w as u32, h as u32, ClockWidget::new);
                view.set_scale_factor(scale * k);
                Content::Clock(view)
            }
            1 => {
                let view = ContentView::new_with(w as u32, h as u32, CalendarPopup::new);
                view.set_scale_factor(scale * k);
                let step = Rc::new(Cell::new(0));
                let slot = step.clone();
                view.ui.on_prev(move || slot.set(slot.get() - 1));
                let slot = step.clone();
                view.ui.on_next(move || slot.set(slot.get() + 1));
                let today = Rc::new(Cell::new(false));
                let slot = today.clone();
                view.ui.on_today(move || slot.set(true));
                let names: Vec<slint::SharedString> = calendar::WEEKDAYS.iter().map(|d| (*d).into()).collect();
                view.ui.set_weekdays(ModelRc::new(VecModel::from(names)));
                let now = LocalTime::now();
                Content::Calendar { view, step, today, shown: Cell::new((i32::from(now.year), u32::from(now.month))) }
            }
            2 => {
                let view = ContentView::new_with(w as u32, h as u32, SystemWidget::new);
                view.set_scale_factor(scale * k);
                Content::System { view, last: Cell::new(system_times()) }
            }
            3 => {
                let view = ContentView::new_with(w as u32, h as u32, NowPlayingWidget::new);
                view.set_scale_factor(scale * k);
                let command = Rc::new(Cell::new(None));
                let slot = command.clone();
                view.ui.on_previous(move || slot.set(Some(media::Command::Previous)));
                let slot = command.clone();
                view.ui.on_play_pause(move || slot.set(Some(media::Command::PlayPause)));
                let slot = command.clone();
                view.ui.on_next(move || slot.set(Some(media::Command::Next)));
                Content::Now { view, command, pending: RefCell::new(None) }
            }
            4 => {
                let view = ContentView::new_with(w as u32, h as u32, StorageWidget::new);
                view.set_scale_factor(scale * k);
                Content::Storage(view)
            }
            5 => {
                let view = ContentView::new_with(w as u32, h as u32, WeatherWidget::new);
                view.set_scale_factor(scale * k);
                Content::Weather { view, pending: RefCell::new(None), place: place.to_string(), loaded: Cell::new(false) }
            }
            7 => {
                let view = ContentView::new_with(w as u32, h as u32, NotesWidget::new);
                view.set_scale_factor(scale * k);
                let text = std::fs::read_to_string(notes_path()).unwrap_or_default();
                view.ui.set_note(text.as_str().into());
                Content::Notes { view, text: RefCell::new(text), editing: Cell::new(false), since: Cell::new(Instant::now()) }
            }
            _ => {
                let view = ContentView::new_with(w as u32, h as u32, CurrencyWidget::new);
                view.set_scale_factor(scale * k);
                Content::Currency(view)
            }
        };
        let panel = glass_plate(win.rect(), scale * k, spec.color).unwrap_or_else(|| panel(w as u32, h as u32, RADIUS_PT * scale * k));
        let mut widget = WidgetWin {
            kind,
            win,
            panel,
            content,
            scale,
            press: None,
            moved: false,
            next_refresh: Instant::now() + interval(kind),
            last_minute: Cell::new(u16::MAX),
            retry_in: Cell::new(None),
            spec,
            context: Cell::new(None),
            calendar_focused: Cell::new(false),
            calendar_previous: Cell::new(None),
            guide: None,
            guide_axes: (false, false),
            guide_visible: false,
        };
        widget.refresh_data();
        widget.draw(true)?;
        Ok(widget)
    }

    /// Стекло под новое место окна (обои под ним другие).
    fn replate(&mut self) -> Result<()> {
        if let Some(p) = glass_plate(self.win.rect(), self.scale * SIZES[self.spec.size.min(2)].2, self.spec.color) {
            self.panel = p;
        }
        self.draw(true)?;
        Ok(())
    }

    /// Забирает правый клик (экранные физические пиксели), если он был.
    pub fn take_context(&self) -> Option<(i32, i32)> {
        self.context.take()
    }

    pub fn spec(&self) -> Spec {
        self.spec
    }

    pub fn focus_calendar(&self) {
        if let Content::Calendar { view, .. } = &self.content {
            if !self.win.is_foreground() {
                let previous = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize };
                self.calendar_previous.set((previous != 0).then_some(previous));
            }
            self.win.focus();
            self.calendar_focused.set(true);
            view.ui.set_keyboard_active(true);
        }
    }

    fn editing_notes(&self) -> bool {
        matches!(&self.content, Content::Notes { editing, .. } if editing.get())
    }

    /// Нажатие на заметки: окно выходит вперёд и получает клавиатуру, пока не потеряет фокус.
    fn start_notes_edit(&self) {
        if let Content::Notes { view, editing, since, .. } = &self.content {
            if !editing.get() {
                editing.set(true);
                since.set(Instant::now());
                view.ui.set_editing(true);
                self.win.focus();
            }
        }
    }

    /// Ввод в заметки: символ или клавиша (Backspace, Enter, Esc).
    fn edit_notes(&self, ch: Option<char>, vk: u32) {
        let Content::Notes { view, text, editing, .. } = &self.content else { return };
        let mut t = text.borrow_mut();
        match (ch, vk) {
            (Some(c), _) => t.push(c),
            (None, 0x08) => {
                t.pop();
            }
            (None, 0x0D) => t.push('\n'),
            (None, 0x1B) => editing.set(false),
            _ => return,
        }
        view.ui.set_note(t.as_str().into());
        view.ui.set_editing(editing.get());
    }

    /// Фокус ушёл: заметки сохраняются, окно уходит под остальные.
    fn stop_notes_edit_if_unfocused(&self) {
        let Content::Notes { view, text, editing, since } = &self.content else { return };
        let escaped = editing.get() && !self.win.is_foreground() && since.get().elapsed() > Duration::from_millis(700);
        if escaped || (!editing.get() && view.ui.get_editing()) {
            editing.set(false);
            view.ui.set_editing(false);
            let _ = std::fs::write(notes_path(), text.borrow().as_str());
            unsafe {
                let _ = SetWindowPos(HWND(self.win.hwnd() as *mut _), Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    /// Положение окна в логических пикселях.
    pub fn position_pt(&self) -> (i32, i32) {
        let r = self.win.rect();
        ((r.x as f32 / self.scale).round() as i32, (r.y as f32 / self.scale).round() as i32)
    }

    /// Обновляет показания: время, календарь, нагрузку.
    fn refresh_data(&mut self) {
        let now = LocalTime::now();
        match &self.content {
            Content::Clock(view) => {
                view.ui.set_time(format!("{}:{:02}", now.hour, now.minute).into());
                view.ui.set_date(format!("{}, {} {}", weekday_name(now.weekday), now.day, month_genitive(now.month)).into());
            }
            Content::Calendar { view, shown, .. } => {
                let (year, month) = shown.get();
                let today = (i32::from(now.year), u32::from(now.month), u32::from(now.day));
                view.ui.set_month_title(calendar::title(year, month).into());
                view.ui.set_other_month((year, month) != (today.0, today.1));
                let days: Vec<CalDay> = calendar::month_grid(year, month, today)
                    .into_iter()
                    .map(|d| CalDay { text: d.day.to_string().into(), in_month: d.in_month, today: d.today })
                    .collect();
                view.ui.set_days(ModelRc::new(VecModel::from(days)));
            }
            Content::System { view, last } => {
                let after = system_times();
                let load = cpu_load(last.get(), after);
                last.set(after);
                view.ui.set_cpu(load);
                view.ui.set_cpu_text(format!("{}%", (load * 100.0).round() as i32).into());
                let (mem, text) = memory_use();
                view.ui.set_memory(mem);
                view.ui.set_memory_text(text.into());
                view.ui.set_memory_percent(format!("{}%", (mem * 100.0).round() as i32).into());
            }
            Content::Now { view, pending, .. } => {
                let mut slot = pending.borrow_mut();
                let received = slot.as_ref().map(|rx| rx.try_recv());
                match received {
                    Some(Ok(state)) => {
                        view.ui.set_active(state.is_some());
                        view.ui.set_playing(state.as_ref().is_some_and(|n| n.playing));
                        view.ui.set_track(state.as_ref().map_or("Ничего не играет".to_string(), |n| n.title.clone()).into());
                        view.ui.set_artist(if state.is_some() { media::subtitle(&state) } else { String::new() }.into());
                        *slot = None;
                    }
                    Some(Err(TryRecvError::Empty)) => self.retry_in.set(Some(Duration::from_millis(300))),
                    _ => {
                        *slot = Some(media::request());
                        self.retry_in.set(Some(Duration::from_millis(300)));
                    }
                }
            }
            Content::Storage(view) => {
                let rows: Vec<DriveInfo> = widgetdata::drives()
                    .into_iter()
                    .take(3)
                    .map(|d| DriveInfo {
                        caption: format!("{} свободно", widgetdata::size_text(d.free)).into(),
                        value: (d.total - d.free) as f32 / d.total as f32,
                        name: d.name.into(),
                    })
                    .collect();
                view.ui.set_drives(ModelRc::new(VecModel::from(rows)));
            }
            Content::Weather { view, pending, place, loaded } => {
                let mut slot = pending.borrow_mut();
                let received = slot.as_ref().map(|rx| rx.try_recv());
                match received {
                    Some(Ok(Some(w))) => {
                        let (text, icon) = widgetdata::wmo(w.code);
                        view.ui.set_city(w.city.as_str().into());
                        view.ui.set_temp(format!("{}°", w.temp).into());
                        view.ui.set_condition(text.into());
                        view.ui.set_range(format!("Макс {}°, мин {}°, ощущается {}°", w.max, w.min, w.feels).into());
                        view.ui.set_sky(icon);
                        view.ui.set_loaded(true);
                        loaded.set(true);
                        *slot = None;
                        self.retry_in.set(Some(Duration::from_secs(30 * 60)));
                    }
                    Some(Ok(None)) | Some(Err(TryRecvError::Disconnected)) => {
                        if !loaded.get() {
                            view.ui.set_note("Нет данных о погоде".into());
                        }
                        *slot = None;
                        self.retry_in.set(Some(Duration::from_secs(120)));
                    }
                    Some(Err(TryRecvError::Empty)) => self.retry_in.set(Some(Duration::from_millis(500))),
                    None => {
                        *slot = Some(widgetdata::request_weather(place));
                        self.retry_in.set(Some(Duration::from_millis(500)));
                    }
                }
            }
            Content::Notes { .. } => {}
            Content::Currency(view) => match currency::widget_rates() {
                Some((date, rates)) => {
                    let rows: Vec<RateRow> = rates
                        .into_iter()
                        .map(|(code, v)| RateRow { code: code.into(), value: format!("{:.2} ₽", v).replace('.', ",").into() })
                        .collect();
                    view.ui.set_rows(ModelRc::new(VecModel::from(rows)));
                    view.ui.set_caption(format!("Курс ЦБ · {}", date.rsplit_once('.').map_or(date.as_str(), |(d, _)| d)).into());
                    self.retry_in.set(Some(Duration::from_secs(10 * 60)));
                }
                None => {
                    view.ui.set_caption("Курс ЦБ · загружаю…".into());
                    self.retry_in.set(Some(Duration::from_secs(10)));
                }
            },
        }
        self.last_minute.set(now.minute);
    }

    fn draw(&self, force: bool) -> Result<()> {
        let content = match on_view!(&self.content, v => v.render_if_changed()) {
            Some(c) => c,
            None if force => on_view!(&self.content, v => v.render()),
            None => return Ok(()),
        };
        let mut out = self.panel.clone();
        cpu::composite_over(&mut out, &content);
        if self.guide_visible {
            if let Some(guide) = &self.guide {
                cpu::composite_over(&mut out, &guide.render());
            }
        }
        self.win.present(&out)
    }

    fn dispatch(&self, ev: &InputEvent) {
        on_view!(&self.content, v => v.dispatch(ev))
    }

    /// Мышь, перетаскивание и обновление. `Some(новое положение)`, когда перетаскивание закончилось.
    pub fn tick(&mut self, neighbors: &[Spec]) -> Result<Option<(i32, i32)>> {
        let mut dropped = None;
        for ev in self.win.take_input() {
            match ev {
                InputEvent::RightUp { x, y } => {
                    let r = self.win.rect();
                    self.context.set(Some((r.x + x, r.y + y)));
                }
                InputEvent::Char(c) if self.editing_notes() => self.edit_notes(Some(c), 0),
                InputEvent::Key { vk, .. } if self.editing_notes() => self.edit_notes(None, vk),
                InputEvent::Key { vk, .. } if self.kind == 1 => {
                    if let Content::Calendar { step, today, .. } = &self.content {
                        match vk {
                            0x25 => step.set(step.get() - 1),
                            0x27 => step.set(step.get() + 1),
                            0x24 => today.set(true),
                            0x1b => {
                                if let Some(previous) = self.calendar_previous.take() { crate::host::bring_to_front(previous); }
                            }
                            _ => {}
                        }
                    }
                }
                InputEvent::Down { x, y } => {
                    let mut at = POINT::default();
                    let _ = unsafe { GetCursorPos(&mut at) };
                    self.press = Some((x, y, at.x, at.y));
                    self.moved = false;
                    self.win.capture_mouse(true);
                    self.dispatch(&ev);
                }
                InputEvent::Move { .. } if self.press.is_some() => {
                    let (_, _, sx, sy) = self.press.unwrap();
                    let mut at = POINT::default();
                    let _ = unsafe { GetCursorPos(&mut at) };
                    if !self.moved && ((at.x - sx).abs() >= DRAG_PX || (at.y - sy).abs() >= DRAG_PX) {
                        self.moved = true;
                        // Нажатие превращается в перетаскивание: интерфейс получает «отпускание» вне окна и клика не будет.
                        self.dispatch(&InputEvent::Move { x: -1000, y: -1000 });
                        self.dispatch(&InputEvent::Up { x: -1000, y: -1000 });
                    }
                    if self.moved {
                        let (gx, gy, _, _) = self.press.unwrap();
                        let x = ((at.x - gx) as f32 / self.scale).round() as i32;
                        let y = ((at.y - gy) as f32 / self.scale).round() as i32;
                        let (x, y, hint, sx, sy) = snap_to_neighbors(Spec { x, y, ..self.spec }, neighbors);
                        self.guide_axes = (sx, sy);
                        self.guide_visible = !hint.is_empty();
                        if self.guide_visible {
                            if self.guide.is_none() {
                                let r = self.win.rect();
                                let view = ContentView::new_with(r.w as u32, r.h as u32, WidgetGuide::new);
                                view.set_scale_factor(self.scale * SIZES[self.spec.size.min(2)].2);
                                self.guide = Some(view);
                            }
                            if let Some(guide) = &self.guide { guide.ui.set_hint(hint.into()); }
                        }
                        self.win.set_position((x as f32 * self.scale).round() as i32, (y as f32 * self.scale).round() as i32)?;
                        self.replate()?;
                    } else {
                        self.dispatch(&ev);
                    }
                }
                InputEvent::Up { .. } => {
                    self.win.capture_mouse(false);
                    if self.moved {
                        let r = self.win.rect();
                        let (raw_x, raw_y) = ((r.x as f32 / self.scale).round() as i32, (r.y as f32 / self.scale).round() as i32);
                        let (x, y) = (if self.guide_axes.0 { raw_x } else { snap(raw_x) }, if self.guide_axes.1 { raw_y } else { snap(raw_y) });
                        self.guide_visible = false;
                        self.guide_axes = (false, false);
                        self.win.set_position((x as f32 * self.scale).round() as i32, (y as f32 * self.scale).round() as i32)?;
                        self.replate()?;
                        dropped = Some((x, y));
                    } else {
                        self.dispatch(&ev);
                        self.start_notes_edit();
                        if self.kind == 1 {
                            self.focus_calendar();
                        }
                    }
                    self.press = None;
                    self.moved = false;
                }
                other => self.dispatch(&other),
            }
        }
        if let Content::Calendar { step, today, shown, .. } = &self.content {
            let delta = step.replace(0);
            let go_today = today.replace(false);
            if go_today || delta != 0 {
                let now = LocalTime::now();
                if go_today {
                    shown.set((i32::from(now.year), u32::from(now.month)));
                } else {
                    let (y, m) = shown.get();
                    shown.set(calendar::shift_month(y, m, delta));
                }
                self.refresh_data();
            }
        }
        if let Content::Now { command, pending, .. } = &self.content {
            if let Some(cmd) = command.take() {
                *pending.borrow_mut() = Some(media::send(cmd));
                self.next_refresh = Instant::now() + Duration::from_millis(500);
            }
        }
        let due = match &self.content {
            Content::Clock(_) | Content::Calendar { .. } => LocalTime::now().minute != self.last_minute.get(),
            _ => Instant::now() >= self.next_refresh,
        };
        if due {
            self.refresh_data();
            self.next_refresh = Instant::now() + self.retry_in.take().unwrap_or_else(|| interval(self.kind));
        }
        self.draw(false)?;
        if self.calendar_focused.get() && !self.win.is_foreground() {
            self.calendar_focused.set(false);
            if let Content::Calendar { view, .. } = &self.content {
                view.ui.set_keyboard_active(false);
            }
            unsafe {
                let _ = SetWindowPos(HWND(self.win.hwnd() as *mut _), Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
        self.stop_notes_edit_if_unfocused();
        if self.press.is_none() {
            crate::wallpaper::trim();
        }
        Ok(dropped)
    }

    /// Через сколько миллисекунд нужен следующий шаг без событий.
    pub fn next_wake_ms(&self) -> u32 {
        match &self.content {
            Content::Clock(_) | Content::Calendar { .. } => crate::clock::ms_until_next_minute(&LocalTime::now()) + 30,
            _ => self.next_refresh.saturating_duration_since(Instant::now()).as_millis() as u32 + 1,
        }
    }

    pub fn animating(&self) -> bool {
        on_view!(&self.content, v => v.animating())
    }
}

fn weekday_name(d: u16) -> &'static str {
    ["Воскресенье", "Понедельник", "Вторник", "Среда", "Четверг", "Пятница", "Суббота"][(d % 7) as usize]
}

fn month_genitive(m: u16) -> &'static str {
    ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"]
        [(m.clamp(1, 12) - 1) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Тестовые данные на текущих обоях, без открытия окон и синтеза ввода.
    #[test]
    #[ignore]
    fn dump_widget_block() {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let dir = std::env::var("MACSHELL_DUMP_DIR").unwrap();
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let wall = crate::wallpaper::screen(sw as u32, sh as u32).expect("wallpaper");
        crate::slint_view::set_accent([0x8a, 0x8f, 0x9b]);
        for scale in [1.0f32, 1.25] {
            let px = |v: f32| (v * scale).round() as u32;
            let mut sheet = crop(&wall, 0, px(48.0), px(570.0), px(360.0));
            let mut place = |x: f32, y: f32, w: f32, h: f32, content: Vec<u32>| {
                let rect = RectI { x: px(x) as i32, y: px(y) as i32, w: px(w) as i32, h: px(h) as i32 };
                let mut plate = glass_plate(rect, scale, 0).unwrap();
                cpu::composite_over(&mut plate, &content);
                let (ox, oy) = (rect.x as u32, rect.y as u32 - px(48.0));
                let mut background = crop(&sheet, ox, oy, rect.w as u32, rect.h as u32);
                cpu::composite_over(&mut background, &plate.pixels);
                for row in 0..rect.h as usize {
                    let dst = (oy as usize + row) * sheet.width as usize + ox as usize;
                    let src = row * rect.w as usize;
                    sheet.pixels[dst..dst + rect.w as usize].copy_from_slice(&background.pixels[src..src + rect.w as usize]);
                }
            };
            let cal = ContentView::new_with(px(280.0), px(320.0), CalendarPopup::new);
            cal.set_scale_factor(scale);
            cal.ui.set_month_title("Ноябрь 2026".into());
            cal.ui.set_weekdays(ModelRc::new(VecModel::from(calendar::WEEKDAYS.iter().map(|d| (*d).into()).collect::<Vec<slint::SharedString>>())));
            cal.ui.set_days(ModelRc::new(VecModel::from(calendar::month_grid(2026, 11, (2026, 11, 8)).into_iter().map(|d| CalDay { text: d.day.to_string().into(), in_month: d.in_month, today: d.today }).collect::<Vec<_>>())));
            place(32.0, 72.0, 280.0, 320.0, cal.render());
            let clock = ContentView::new_with(px(220.0), px(152.0), ClockWidget::new);
            clock.set_scale_factor(scale);
            clock.ui.set_time("12:09".into());
            clock.ui.set_date("Воскресенье, 8 ноября".into());
            place(328.0, 72.0, 220.0, 152.0, clock.render());
            let rates = ContentView::new_with(px(220.0), px(152.0), CurrencyWidget::new);
            rates.set_scale_factor(scale);
            rates.ui.set_caption("Курс ЦБ · 07.11".into());
            rates.ui.set_rows(ModelRc::new(VecModel::from(vec![RateRow { code: "USD".into(), value: "82,14 ₽".into() }, RateRow { code: "EUR".into(), value: "92,19 ₽".into() }, RateRow { code: "CNY".into(), value: "12,27 ₽".into() }])));
            place(328.0, 240.0, 220.0, 152.0, rates.render());
            crate::snapshot::write_bmp(&format!("{dir}\\widget_block_{}.bmp", (scale * 100.0) as u32), &sheet).unwrap();
            let guide = ContentView::new_with(px(220.0), px(152.0), WidgetGuide::new);
            guide.set_scale_factor(scale);
            guide.ui.set_hint("Общий край · отступ 16".into());
            let (x, y) = (px(328.0), px(24.0));
            let mut region = crop(&sheet, x, y, px(220.0), px(152.0));
            cpu::composite_over(&mut region, &guide.render());
            for row in 0..region.height as usize {
                let dst = (y as usize + row) * sheet.width as usize + x as usize;
                let src = row * region.width as usize;
                sheet.pixels[dst..dst + region.width as usize].copy_from_slice(&region.pixels[src..src + region.width as usize]);
            }
            crate::snapshot::write_bmp(&format!("{dir}\\widget_guide_{}.bmp", (scale * 100.0) as u32), &sheet).unwrap();
        }
    }

    #[test]
    fn calendar_and_compact_widgets_form_a_balanced_block() {
        let (_, calendar_height) = size_pt(1);
        let (_, clock_height) = size_pt(0);
        let (_, currency_height) = size_pt(6);
        assert_eq!((calendar_height, clock_height, currency_height), (320.0, 152.0, 152.0));
        assert_eq!(calendar_height, clock_height + 16.0 + currency_height);
    }

    #[test]
    fn neighbor_snap_aligns_edges_and_sixteen_point_gaps_without_reaching_distant_widgets() {
        let other = Spec::new(1, 32, 72);
        let moving = Spec::new(0, 329, 74);
        let (x, y, hint, _, _) = snap_to_neighbors(moving, &[other]);
        assert_eq!((x, y), (328, 72));
        assert!(hint.contains("16") && hint.contains("край"));
        let distant = Spec::new(0, 329, 900);
        assert_eq!(snap_to_neighbors(distant, &[other]), (329, 900, String::new(), false, false));
    }

    #[test]
    fn aligns_only_the_recognized_previous_widget_arrangement() {
        let mut lines = vec!["calendar,32,72".into(), "clock,328,72".into(), "currency,328,232,m,graphite".into(), "notes,560,72".into()];
        assert!(align_legacy_triplet(&mut lines));
        assert_eq!(entries(&lines).iter().find(|s| s.kind == 6).unwrap().y, 240);
        assert_eq!(entries(&lines).iter().find(|s| s.kind == 6).unwrap().color, 7);
        assert!(!align_legacy_triplet(&mut lines));
        let mut custom = vec!["calendar,32,72".into(), "clock,328,72".into(), "currency,328,234".into()];
        assert!(!align_legacy_triplet(&mut custom));
    }

    #[test]
    fn widget_lines_parse_and_roundtrip() {
        assert_eq!(parse("clock,40,64"), Some(Spec::new(0, 40, 64)));
        assert_eq!(parse(" Calendar , -8 , 16 "), Some(Spec::new(1, -8, 16)));
        assert_eq!(parse("bogus,1,2"), None);
        assert_eq!(parse("clock,1"), None);
        assert_eq!(parse(&serialize(&Spec::new(2, 8, 24))), Some(Spec::new(2, 8, 24)));
        assert_eq!(serialize(&Spec::new(2, 8, 24)), "system,8,24", "defaults are not written");
    }

    #[test]
    fn size_and_color_are_optional_and_roundtrip() {
        let spec = parse("weather,16,32,l,blue").unwrap();
        assert_eq!((spec.size, spec.color), (2, 1));
        assert_eq!(serialize(&spec), "weather,16,32,l,blue");
        assert_eq!(parse("clock,1,2,s").unwrap().size, 0);
        assert_eq!(serialize(&Spec { color: 3, ..Spec::new(0, 1, 2) }), "clock,1,2,m,pink", "a color needs the size slot");
        assert_eq!(parse("clock,1,2,huge,mauve").map(|s| (s.size, s.color)), Some((1, 0)), "unknown words fall back");
    }

    #[test]
    fn bigger_sizes_scale_the_window() {
        let spec = Spec { size: 2, ..Spec::new(0, 0, 0) };
        assert_eq!(spec.window_pt(), (308.0, 213.0));
        assert_eq!(Spec { size: 0, ..Spec::new(0, 0, 0) }.window_pt(), (165.0, 114.0));
    }

    #[test]
    fn config_list_keeps_one_line_per_widget_and_remembers_size_and_color() {
        let mut list = vec!["clock,8,8".to_string(), "junk".to_string(), "clock,99,99".to_string()];
        assert_eq!(entries(&list), vec![Spec::new(0, 8, 8)], "junk and repeats are ignored");
        set_size(&mut list, 0, 2);
        set_color(&mut list, 0, 4);
        upsert(&mut list, 0, 16, 24);
        upsert(&mut list, 2, 40, 48);
        let all = entries(&list);
        assert_eq!(all.iter().find(|s| s.kind == 0), Some(&Spec { size: 2, color: 4, ..Spec::new(0, 16, 24) }), "moving keeps size and color");
        assert_eq!(all.iter().find(|s| s.kind == 2), Some(&Spec::new(2, 40, 48)));
        remove(&mut list, 0);
        assert_eq!(entries(&list), vec![Spec::new(2, 40, 48)]);
        set_size(&mut list, 5, 1); // такого виджета нет: список не меняется
        assert_eq!(entries(&list).len(), 1);
    }

    #[test]
    fn positions_snap_to_the_grid_and_new_widgets_do_not_stack() {
        assert_eq!((snap(13), snap(11), snap(-5), snap(0)), (16, 8, -8, 0));
        let first = default_position(0, 0, 1920.0);
        let second = default_position(1, 0, 1920.0);
        assert!(second.1 > first.1 + 100 && second.0 < first.0);
        assert!(first.0 % 8 == 0 && first.1 % 8 == 0);
    }

    #[test]
    fn panel_is_round_translucent_and_clear_at_the_corners() {
        let p = panel(100, 60, 22.0);
        assert_eq!(p.pixels[0] >> 24, 0, "the very corner is transparent");
        let mid = p.pixels[30 * 100 + 50];
        assert!(mid >> 24 > 120 && mid >> 24 < 200, "inside is dark and translucent: {:#x}", mid >> 24);
        let edge_row = p.pixels[30 * 100];
        assert!(edge_row >> 24 > 0, "the straight edge is painted");
    }

    #[test]
    fn cpu_load_from_two_snapshots() {
        // Простой 600 из 1000 тиков: занято 40 %.
        assert!((cpu_load((0, 0, 0), (600, 400, 600)) - 0.4).abs() < 1e-6);
        assert_eq!(cpu_load((5, 5, 5), (5, 5, 5)), 0.0, "no time passed");
    }

    #[test]
    fn memory_is_readable() {
        let (share, text) = memory_use();
        assert!((0.0..=1.0).contains(&share) && text.contains("ГБ"), "{share} {text}");
    }

    #[test]
    fn names_cover_every_weekday_and_month() {
        assert_eq!(weekday_name(0), "Воскресенье");
        assert_eq!(weekday_name(6), "Суббота");
        assert_eq!(month_genitive(11), "ноября");
        assert_eq!(month_genitive(12), "декабря");
    }
}
