//! Окно «Приложения»: сетка значков всех установленных приложений с фильтрами. Создаётся при открытии и
//! уничтожается при закрытии. Значки подгружаются только для видимых рядов, остальные ждут прокрутки.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use capture::{RectI, capture_region};
use glass::{Bitmap, GlassParams, cpu};
use slint::{Image, Model, ModelRc, VecModel};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, IsWindow, SM_CXSCREEN, SM_CYSCREEN, SetForegroundWindow, WM_ACTIVATE,
};

use crate::anim::{ease_out_cubic, progress};
use crate::bar_style::{SoftBackdrop, padded_rect};
use crate::host::{InputEvent, LayeredWindow, primary_scale};
use crate::icons::{IconPixels, load_icon};
use crate::index::{Entry, Kind, Usage, rank};
use crate::slint_view::{Apps, AppTile, ContentView, image_from_pixels};
use crate::spotlight::{Launch, Outcome, delete_word};

const WIDTH_PT: f32 = 820.0;
const HEIGHT_PT: f32 = 600.0;
pub const COLUMNS: usize = 6;
pub const CELL_H_PT: f32 = 118.0;
/// Высота видимой части сетки: окно минус шапка (118) и нижний отступ (22).
pub const GRID_H_PT: f32 = HEIGHT_PT - 118.0 - 22.0;
const ICON_PT: f32 = 64.0;
const WHEEL_STEP_PT: f32 = 72.0;
const APPEAR_MS: u32 = 140;
const APPEAR_DY_PT: f32 = 10.0;
const FOCUS_RETRY_MS: u64 = 40;
/// Сколько значков догружать за один кадр: окно остаётся отзывчивым.
const ICONS_PER_TICK: usize = 10;
const FREQUENT_MAX: usize = 30;

const VK_ESCAPE: u32 = 0x1B;
const VK_RETURN: u32 = 0x0D;
const VK_BACK: u32 = 0x08;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
/// Сколько результатов поиска показывается в сетке.
const SEARCH_MAX: usize = 120;

pub const CHIPS: [&str; 5] = ["Все", "Частые", "Игры", "Microsoft", "Другие"];
const CHIP_ALL: usize = 0;
const CHIP_FREQUENT: usize = 1;
const CHIP_GAMES: usize = 2;
const CHIP_MICROSOFT: usize = 3;

fn is_game(e: &Entry) -> bool {
    let t = e.target.to_lowercase();
    t.contains("steam://") || t.contains("\\games\\") || t.contains("epic games") || t.contains("epicgames") || t.contains("riot")
        || t.contains("battle.net") || t.contains("minecraft") || t.contains("\\steamapps\\")
}

fn is_microsoft(e: &Entry) -> bool {
    let t = e.target.to_lowercase();
    t.contains("microsoft.") || t.contains("windows") || t.contains("msedge") || e.title.starts_with("Microsoft ")
}

/// Приложения для выбранного фильтра, по алфавиту (частые: по числу запусков).
pub fn entries_for(index: &[Entry], usage: &Usage, chip: usize) -> Vec<Entry> {
    let mut apps: Vec<Entry> = index.iter().filter(|e| e.kind == Kind::App).cloned().collect();
    match chip {
        CHIP_FREQUENT => {
            apps.retain(|e| usage.count(&e.target) > 0);
            apps.sort_by(|a, b| usage.count(&b.target).cmp(&usage.count(&a.target)).then_with(|| a.norm.cmp(&b.norm)));
            apps.truncate(FREQUENT_MAX);
            return apps;
        }
        CHIP_GAMES => apps.retain(is_game),
        CHIP_MICROSOFT => apps.retain(|e| is_microsoft(e) && !is_game(e)),
        CHIP_ALL => {}
        _ => apps.retain(|e| !is_game(e) && !is_microsoft(e)),
    }
    apps.sort_by(|a, b| a.norm.cmp(&b.norm));
    apps
}

/// Поиск по всем приложениям, как в поле Launchpad: фильтры не действуют, порядок по близости к запросу.
pub fn search_apps(index: &[Entry], usage: &Usage, query: &str) -> Vec<Entry> {
    let apps = entries_for(index, usage, CHIP_ALL);
    rank(query, &apps, usage, SEARCH_MAX).into_iter().map(|(i, _)| apps[i].clone()).collect()
}

/// Куда сдвигается выбор с клавиатуры. Первое нажатие стрелки выбирает первую плитку; на краях выбор стоит
/// на месте, а из неполного нижнего ряда «вниз» ведёт на последнюю плитку.
pub fn move_selection(selected: Option<usize>, vk: u32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let Some(i) = selected.filter(|&i| i < count) else { return Some(0) };
    let next = match vk {
        VK_LEFT => i.saturating_sub(1),
        VK_RIGHT => (i + 1).min(count - 1),
        VK_UP if i >= COLUMNS => i - COLUMNS,
        VK_DOWN if i / COLUMNS < (count - 1) / COLUMNS => (i + COLUMNS).min(count - 1),
        _ => i,
    };
    Some(next)
}

/// Прокрутка, при которой плитка `index` видна целиком.
pub fn scroll_to_show(scroll: f32, index: usize) -> f32 {
    let top = (index / COLUMNS) as f32 * CELL_H_PT;
    let bottom = top + CELL_H_PT;
    if top < scroll {
        top
    } else if bottom > scroll + GRID_H_PT {
        bottom - GRID_H_PT
    } else {
        scroll
    }
}

/// Насколько можно прокрутить сетку из `count` плиток (в логических пикселях).
pub fn max_scroll(count: usize) -> f32 {
    let rows = count.div_ceil(COLUMNS) as f32;
    (rows * CELL_H_PT - GRID_H_PT).max(0.0)
}

/// Номера плиток в видимых рядах и ряду выше и ниже: их значки стоит иметь наготове.
pub fn visible_tiles(scroll: f32, count: usize) -> Range<usize> {
    let first_row = ((scroll / CELL_H_PT).floor() as i64 - 1).max(0) as usize;
    let last_row = ((scroll + GRID_H_PT) / CELL_H_PT).ceil() as usize + 1;
    (first_row * COLUMNS).min(count)..(last_row * COLUMNS).min(count)
}

fn apps_glass() -> GlassParams {
    GlassParams {
        radius: 36.0,
        bevel: 26.0,
        refraction: 16.0,
        blur_radius: 14,
        saturation: 1.5,
        tint: [0.05, 0.06, 0.09, 0.42],
        dispersion: 2.0,
        rim: 0.6,
        ..GlassParams::default()
    }
}

/// Значки, уже прочитанные у системы: первый показ сетки читал их все на глазах у пользователя (около 0,2 с), теперь
/// их заранее читает фоновый поток (`warm`), а повторные открытия берут готовое.
static ICON_CACHE: Mutex<Option<HashMap<(String, i32), Option<IconPixels>>>> = Mutex::new(None);

fn cached_icon(target: &str, px: i32) -> Option<Option<IconPixels>> {
    ICON_CACHE.lock().ok()?.as_ref()?.get(&(target.to_string(), px)).cloned()
}

fn icon_pixels(target: &str, px: i32) -> Option<IconPixels> {
    if let Some(hit) = cached_icon(target, px) {
        return hit;
    }
    let loaded = load_icon(target, px);
    if let Ok(mut c) = ICON_CACHE.lock() {
        c.get_or_insert_with(HashMap::new).insert((target.to_string(), px), loaded.clone());
    }
    loaded
}

/// Стекло прошлого открытия: показывается сразу, пока считается свежее (обои меняются редко).
static LAST_GLASS: Mutex<Option<Bitmap>> = Mutex::new(None);

/// Окно сетки на экране `sw` x `sh` при масштабе `scale`.
fn window_rect(scale: f32, sw: i32, sh: i32) -> RectI {
    let (w, h) = ((WIDTH_PT * scale).round() as i32, (HEIGHT_PT * scale).round() as i32);
    RectI { x: (sw - w) / 2, y: ((sh - h) as f32 * 0.45).round() as i32, w, h }
}

/// Размытый снимок экрана под окном и стекло по нему. Тяжёлая часть открытия, поэтому идёт в фоне.
fn compute_glass(rect: RectI, scale: f32, sw: i32, sh: i32) -> Result<Bitmap> {
    let params = apps_glass().scaled(scale);
    let padded = padded_rect(rect, 2 * params.blur_radius as i32, sw, sh);
    let shot = capture_region(padded)?;
    let blurred = SoftBackdrop::new(&shot, params.blur_radius);
    let inner = blurred.region((rect.x - padded.x) as u32, (rect.y - padded.y) as u32, rect.w as u32, rect.h as u32);
    let glass = cpu::render(&inner, &GlassParams { blur_radius: 0, ..params });
    if let Ok(mut last) = LAST_GLASS.lock() {
        *last = Some(glass.clone());
    }
    Ok(glass)
}

/// Готовит открытие заранее, в фоне: значки первых приложений и стекло. Вызывается, когда собран список приложений.
pub fn warm(index: &[Entry]) {
    let targets: Vec<String> = index.iter().filter(|e| e.kind == Kind::App).take(WARM_ICONS).map(|e| e.target.clone()).collect();
    let _ = std::thread::Builder::new().name("apps-warm".into()).spawn(move || {
        let scale = primary_scale();
        let px = (ICON_PT * scale).round() as i32;
        for t in &targets {
            let _ = icon_pixels(t, px);
        }
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let have = LAST_GLASS.lock().ok().is_some_and(|g| g.is_some());
        if !have {
            let _ = compute_glass(window_rect(scale, sw, sh), scale, sw, sh);
        }
    });
}

/// Сколько значков читается заранее.
const WARM_ICONS: usize = 60;
/// Значков, которые открытие читает само, если заранее их не прочитали; остальные догружает `tick`.
const ICONS_AT_OPEN: usize = 8;

pub struct AppsWin {
    win: LayeredWindow,
    view: ContentView<Apps>,
    scale: f32,
    glass: Bitmap,
    /// Свежее стекло, которое считается в фоне; пока его нет, стоит стекло прошлого открытия.
    glass_rx: Option<Receiver<Bitmap>>,
    glass_dirty: bool,
    frame: Bitmap,
    index: Arc<Vec<Entry>>,
    usage: Usage,
    chip: usize,
    /// Набранный запрос: пока он не пуст, сетка показывает результаты поиска.
    query: String,
    /// Плитка, выбранная стрелками; Enter запускает её.
    selected: Option<usize>,
    shown: Vec<Entry>,
    tiles: Rc<VecModel<AppTile>>,
    icons: HashMap<String, Option<Image>>,
    /// Какие плитки уже получили значок (или его отсутствие).
    loaded: Vec<bool>,
    scroll: f32,
    clicked: Rc<Cell<i32>>,
    chosen: Rc<Cell<i32>>,
    /// Плитка, у которой нажали крестик; `handle_input` превращает её в `removed`.
    remove_clicked: Rc<Cell<i32>>,
    removed: Option<String>,
    appear: Option<Instant>,
    previous: isize,
    restore_focus: bool,
    focus_retries: u32,
    next_focus_at: Instant,
    had_focus: bool,
}

impl AppsWin {
    pub fn open(index: Arc<Vec<Entry>>, usage: &Usage) -> Result<AppsWin> {
        let scale = primary_scale();
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let rect = window_rect(scale, sw, sh);
        let (w, h) = (rect.w, rect.h);
        let previous = unsafe { GetForegroundWindow() }.0 as isize;

        let mut lap_t = Instant::now();
        let mut lap = |what: &str| {
            if cfg!(test) {
                eprintln!("  {what}: {:?}", lap_t.elapsed());
            }
            lap_t = Instant::now();
        };
        let mut win = LayeredWindow::new_focusable(rect)?;
        let motion = crate::anim::enabled();
        win.animate_overlay(false, -(APPEAR_DY_PT * scale).round() as i32);
        if motion { win.set_blend(0, -(APPEAR_DY_PT * scale).round() as i32)?; }
        lap("window");
        let view = ContentView::new_with(w as u32, h as u32, Apps::new);
        lap("view");
        view.set_scale_factor(scale);
        let clicked = Rc::new(Cell::new(-1));
        let slot = clicked.clone();
        view.ui.on_activated(move |i| slot.set(i));
        let chosen = Rc::new(Cell::new(-1));
        let slot = chosen.clone();
        view.ui.on_chip_selected(move |i| slot.set(i));
        let remove_clicked = Rc::new(Cell::new(-1));
        let slot = remove_clicked.clone();
        view.ui.on_remove_requested(move |i| slot.set(i));
        let chips: Vec<slint::SharedString> = CHIPS.iter().map(|c| (*c).into()).collect();
        view.ui.set_chips(ModelRc::new(VecModel::from(chips)));

        // Стекло: сразу прошлое (или простая тёмная плита при первом открытии), свежее считается в фоне.
        let glass = LAST_GLASS.lock().ok().and_then(|g| g.clone()).filter(|g| (g.width, g.height) == (w as u32, h as u32));
        let glass = glass.unwrap_or_else(|| crate::widgets::panel(w as u32, h as u32, apps_glass().radius * scale));
        let (tx, glass_rx) = channel();
        let _ = std::thread::Builder::new().name("apps-glass".into()).spawn(move || {
            if let Ok(fresh) = compute_glass(rect, scale, sw, sh) {
                let _ = tx.send(fresh);
            }
        });
        lap("glass");

        let tiles = Rc::new(VecModel::from(Vec::<AppTile>::new()));
        view.ui.set_tiles(ModelRc::from(tiles.clone()));
        let mut apps = AppsWin {
            win,
            view,
            scale,
            glass,
            glass_rx: Some(glass_rx),
            glass_dirty: false,
            frame: Bitmap::new(w as u32, h as u32),
            index,
            usage: usage.clone(),
            chip: CHIP_ALL,
            query: String::new(),
            selected: None,
            shown: Vec::new(),
            tiles,
            icons: HashMap::new(),
            loaded: Vec::new(),
            scroll: 0.0,
            clicked,
            chosen,
            remove_clicked,
            removed: None,
            appear: motion.then(Instant::now),
            previous,
            restore_focus: true,
            focus_retries: 12,
            next_focus_at: Instant::now() + Duration::from_millis(FOCUS_RETRY_MS),
            had_focus: false,
        };
        apps.rebuild();
        lap("rebuild");
        apps.load_icons(ICONS_AT_OPEN);
        lap("icons");
        apps.present()?;
        lap("present");
        apps.win.focus();
        lap("focus");
        apps.win.capture_mouse(true);
        Ok(apps)
    }

    /// Новый индекс (фоновая сборка закончилась, пока окно открыто).
    pub fn set_index(&mut self, index: Arc<Vec<Entry>>) {
        self.index = index;
        self.rebuild();
    }

    /// Адрес приложения, которое пользователь убрал крестиком; забирается один раз.
    pub fn take_removed(&mut self) -> Option<String> {
        self.removed.take()
    }

    pub fn shown_count(&self) -> usize {
        self.shown.len()
    }

    pub fn chip(&self) -> usize {
        self.chip
    }

    pub fn select_chip(&mut self, chip: usize) {
        if chip < CHIPS.len() && (chip != self.chip || !self.query.is_empty()) {
            self.chip = chip;
            self.query.clear();
            self.selected = None;
            self.scroll = 0.0;
            self.rebuild();
        }
    }

    /// Запрос изменился: сетка собирается заново сверху, выбрана первая плитка (Enter запускает лучший результат).
    fn query_changed(&mut self) {
        self.scroll = 0.0;
        self.selected = if self.query.is_empty() { None } else { Some(0) };
        self.rebuild();
    }

    /// Выбор сдвинут стрелкой: сетка прокручивается, чтобы выбранная плитка была видна.
    fn select_by_key(&mut self, vk: u32) {
        self.selected = move_selection(self.selected, vk, self.shown.len());
        self.view.ui.set_selected(self.selected.map_or(-1, |i| i as i32));
        if let Some(i) = self.selected {
            let to = scroll_to_show(self.scroll, i).clamp(0.0, max_scroll(self.shown.len()));
            self.scroll_by(to - self.scroll);
        }
    }

    fn rebuild(&mut self) {
        self.shown = if self.query.is_empty() {
            entries_for(&self.index, &self.usage, self.chip)
        } else {
            search_apps(&self.index, &self.usage, &self.query)
        };
        self.selected = self.selected.filter(|&i| i < self.shown.len());
        self.view.ui.set_query(self.query.as_str().into());
        self.view.ui.set_selected(self.selected.map_or(-1, |i| i as i32));
        let rows: Vec<AppTile> = self
            .shown
            .iter()
            .map(|e| match self.icons.get(&e.target) {
                Some(icon) => AppTile { name: e.title.as_str().into(), icon: icon.clone().unwrap_or_default(), has_icon: icon.is_some() },
                None => AppTile { name: e.title.as_str().into(), icon: Image::default(), has_icon: false },
            })
            .collect();
        self.loaded = self.shown.iter().map(|e| self.icons.contains_key(&e.target)).collect();
        self.tiles.set_vec(rows);
        // Во время поиска ни один фильтр не выбран.
        self.view.ui.set_chip(if self.query.is_empty() { self.chip as i32 } else { -1 });
        // После удаления плитки сетка остаётся на месте (только не уезжает за новый конец).
        self.scroll = self.scroll.min(max_scroll(self.shown.len()));
        self.view.ui.set_scroll_to(self.scroll);
        self.view.ui.set_empty_text(if !self.query.is_empty() {
            "Ничего не найдено".into()
        } else if self.chip == CHIP_FREQUENT { "Запущенные приложения появятся здесь".into() } else { "Здесь пока ничего нет".into() });
    }

    /// Догружает значки видимых плиток; не больше `limit` за вызов. `true`, если что-то осталось.
    fn load_icons(&mut self, limit: usize) -> bool {
        let px = (ICON_PT * self.scale).round() as i32;
        let mut done = 0;
        for i in visible_tiles(self.scroll, self.shown.len()) {
            if self.loaded[i] {
                continue;
            }
            // Готовый значок из общего кэша ничего не стоит и в лимит не входит.
            let ready = self.icons.contains_key(&self.shown[i].target) || cached_icon(&self.shown[i].target, px).is_some();
            if !ready && done >= limit {
                return true;
            }
            let target = self.shown[i].target.clone();
            let icon = self.icons.entry(target).or_insert_with(|| icon_pixels(&self.shown[i].target, px).map(|p| image_from_pixels(&p))).clone();
            let mut row = self.tiles.row_data(i).unwrap_or_default();
            row.has_icon = icon.is_some();
            row.icon = icon.unwrap_or_default();
            self.tiles.set_row_data(i, row);
            self.loaded[i] = true;
            if !ready {
                done += 1;
            }
        }
        false
    }

    fn scroll_by(&mut self, delta_pt: f32) {
        let next = (self.scroll + delta_pt).clamp(0.0, max_scroll(self.shown.len()));
        if next != self.scroll {
            self.scroll = next;
            self.view.ui.set_scroll_to(next);
        }
    }

    pub fn handle_input(&mut self) -> Outcome {
        let (w, h) = (self.win.rect().w, self.win.rect().h);
        for ev in self.win.take_input() {
            match ev {
                // Esc сначала стирает запрос, как поле поиска в Launchpad, и только потом закрывает окно.
                InputEvent::Key { vk: VK_ESCAPE, .. } => {
                    if self.query.is_empty() {
                        return Outcome::Close;
                    }
                    self.query.clear();
                    self.query_changed();
                }
                InputEvent::Char(c) => {
                    self.query.push(c);
                    self.query_changed();
                }
                InputEvent::Key { vk: VK_BACK, ctrl, .. } => {
                    if ctrl {
                        delete_word(&mut self.query);
                    } else {
                        self.query.pop();
                    }
                    self.query_changed();
                }
                InputEvent::Key { vk: VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN, .. } if !self.shown.is_empty() => {
                    if let InputEvent::Key { vk, .. } = ev {
                        self.select_by_key(vk);
                    }
                }
                InputEvent::Key { vk: VK_RETURN, .. } => {
                    if let Some(e) = self.selected.and_then(|i| self.shown.get(i)) {
                        return Outcome::Launch(Launch::Entry(e.clone()));
                    }
                }
                InputEvent::Wheel { delta } => {
                    // Колесо вперёд (положительное) листает вверх.
                    self.scroll_by(-(delta as f32) / 120.0 * WHEEL_STEP_PT);
                }
                InputEvent::Down { x, y } => {
                    if x < 0 || y < 0 || x >= w || y >= h {
                        return Outcome::Close;
                    }
                    self.view.dispatch(&ev);
                }
                InputEvent::Message { msg, .. } if msg == WM_ACTIVATE => {
                    if self.win.is_foreground() {
                        self.had_focus = true;
                    } else if self.had_focus {
                        return Outcome::Close;
                    }
                }
                other => self.view.dispatch(&other),
            }
        }
        let chosen = self.chosen.replace(-1);
        if chosen >= 0 {
            self.select_chip(chosen as usize);
        }
        let remove = self.remove_clicked.replace(-1);
        if remove >= 0 {
            self.removed = self.shown.get(remove as usize).map(|e| e.target.clone());
            // Клик по крестику плитку не запускает.
            self.clicked.set(-1);
            return Outcome::Stay;
        }
        let clicked = self.clicked.replace(-1);
        if clicked >= 0 {
            if let Some(e) = self.shown.get(clicked as usize) {
                return Outcome::Launch(Launch::Entry(e.clone()));
            }
        }
        Outcome::Stay
    }

    pub fn present(&mut self) -> Result<()> {
        let content = match self.view.render_if_changed() {
            Some(c) => c,
            None if self.glass_dirty => self.view.render(),
            None => return Ok(()),
        };
        self.glass_dirty = false;
        self.frame.pixels.copy_from_slice(&self.glass.pixels);
        cpu::composite_over(&mut self.frame, &content);
        self.win.present(&self.frame)
    }

    /// Появление окна, догрузка значков, фокус. `true`, если нужен ещё кадр.
    pub fn tick(&mut self) -> Result<bool> {
        let mut animating = false;
        if self.win.is_foreground() {
            self.had_focus = true;
        }
        if self.focus_retries > 0 {
            if self.had_focus {
                self.focus_retries = 0;
            } else if Instant::now() >= self.next_focus_at {
                self.win.focus();
                self.focus_retries -= 1;
                self.next_focus_at = Instant::now() + Duration::from_millis(FOCUS_RETRY_MS);
            }
        }
        if let Some(start) = self.appear {
            let t = ease_out_cubic(progress(start, APPEAR_MS));
            let dy = (-(APPEAR_DY_PT * self.scale) * (1.0 - t)).round() as i32;
            self.win.set_blend((255.0 * t).round() as u8, dy)?;
            if t >= 1.0 {
                self.appear = None;
            } else {
                animating = true;
            }
        }
        let fresh = self.glass_rx.as_ref().map(|rx| rx.try_recv());
        match fresh {
            Some(Ok(glass)) => {
                self.glass = glass;
                self.glass_dirty = true;
                self.glass_rx = None;
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => self.glass_rx = None,
            _ => {}
        }
        let more_icons = self.load_icons(ICONS_PER_TICK);
        Ok(animating || more_icons || self.glass_rx.is_some() || self.view.animating())
    }

    /// Через сколько миллисекунд нужен следующий кадр, если ничего не происходит (повтор захвата фокуса).
    pub fn next_wake_ms(&self) -> u32 {
        if self.focus_retries > 0 {
            self.next_focus_at.saturating_duration_since(Instant::now()).as_millis() as u32 + 1
        } else {
            u32::MAX
        }
    }

    /// Закрытие после запуска: фокус отдаёт запущенное приложение.
    pub fn launched(&mut self) {
        self.restore_focus = false;
    }
}

impl Drop for AppsWin {
    fn drop(&mut self) {
        self.win.capture_mouse(false);
        if self.restore_focus && self.win.is_foreground() && self.previous != 0 {
            unsafe {
                let prev = HWND(self.previous as *mut _);
                if IsWindow(Some(prev)).as_bool() {
                    let _ = SetForegroundWindow(prev);
                }
            }
        }
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    /// Замер открытия сетки: печатает, сколько занимает каждый этап. Нужен рабочий стол.
    #[test]
    #[ignore = "opens a real window and prints timings"]
    fn open_timing() {
        let t = Instant::now();
        let index = Arc::new(crate::index::build_index());
        eprintln!("index: {} entries in {:?}", index.len(), t.elapsed());
        for round in 0..3 {
            let t = Instant::now();
            let apps = AppsWin::open(index.clone(), &Usage::load()).expect("open");
            eprintln!("open #{round}: {:?}", t.elapsed());
            drop(apps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(title: &str, target: &str) -> Entry {
        Entry {
            title: title.into(),
            norm: title.to_lowercase(),
            keywords: vec![],
            target: target.into(),
            kind: Kind::App,
            glyph: "",
        }
    }

    fn sample() -> Vec<Entry> {
        vec![
            app("Zed", "shell:AppsFolder\\{X}\\Zed\\zed.exe"),
            app("Dota 2", "shell:AppsFolder\\steam://rungameid/570"),
            app("Paint", "shell:AppsFolder\\Microsoft.Paint_8wekyb3d8bbwe!App"),
            app("Blender", "shell:AppsFolder\\{X}\\Blender\\blender.exe"),
            Entry { kind: Kind::Setting, ..app("Звук", "ms-settings:sound") },
        ]
    }

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.title.as_str()).collect()
    }

    #[test]
    fn all_lists_apps_only_in_alphabetical_order() {
        let list = entries_for(&sample(), &Usage::default(), CHIP_ALL);
        assert_eq!(names(&list), vec!["Blender", "Dota 2", "Paint", "Zed"]);
    }

    #[test]
    fn category_chips_split_games_microsoft_and_the_rest() {
        let s = sample();
        let u = Usage::default();
        assert_eq!(names(&entries_for(&s, &u, CHIP_GAMES)), vec!["Dota 2"]);
        assert_eq!(names(&entries_for(&s, &u, CHIP_MICROSOFT)), vec!["Paint"]);
        assert_eq!(names(&entries_for(&s, &u, 4)), vec!["Blender", "Zed"]);
    }

    #[test]
    fn frequent_chip_orders_by_launch_count_and_skips_unused() {
        let s = sample();
        let mut u = Usage::default();
        u.bump(&s[0].target);
        u.bump(&s[3].target);
        u.bump(&s[3].target);
        assert_eq!(names(&entries_for(&s, &u, CHIP_FREQUENT)), vec!["Blender", "Zed"]);
        assert!(entries_for(&s, &Usage::default(), CHIP_FREQUENT).is_empty());
    }

    #[test]
    fn search_ignores_filters_and_settings_and_orders_by_match() {
        let found = search_apps(&sample(), &Usage::default(), "pai");
        assert_eq!(names(&found), vec!["Paint"]);
        assert!(search_apps(&sample(), &Usage::default(), "звук").is_empty(), "settings pages are not apps");
        assert!(search_apps(&sample(), &Usage::default(), "qqqq").is_empty());
    }

    #[test]
    fn arrows_start_at_the_first_tile_and_stop_at_the_edges() {
        let n = 8; // 6 в первом ряду, 2 во втором
        assert_eq!(move_selection(None, VK_RIGHT, n), Some(0));
        assert_eq!(move_selection(None, VK_UP, n), Some(0));
        assert_eq!(move_selection(Some(0), VK_LEFT, n), Some(0));
        assert_eq!(move_selection(Some(7), VK_RIGHT, n), Some(7));
        assert_eq!(move_selection(Some(2), VK_RIGHT, n), Some(3));
        assert_eq!(move_selection(Some(2), VK_UP, n), Some(2), "top row: nowhere to go");
        assert_eq!(move_selection(Some(7), VK_UP, n), Some(1));
        assert_eq!(move_selection(Some(1), VK_DOWN, n), Some(7));
        assert_eq!(move_selection(Some(5), VK_DOWN, n), Some(7), "short last row: land on the last tile");
        assert_eq!(move_selection(Some(7), VK_DOWN, n), Some(7), "last row stays");
        assert_eq!(move_selection(Some(3), VK_LEFT, 0), None);
    }

    #[test]
    fn scrolling_follows_the_selected_row() {
        assert_eq!(scroll_to_show(0.0, 0), 0.0);
        assert_eq!(scroll_to_show(0.0, 5), 0.0, "first row visible");
        // Ряд 4 (индекс 24) заканчивается на 5*118 = 590 > 460: прокрутка поднимается.
        assert_eq!(scroll_to_show(0.0, 24), 5.0 * CELL_H_PT - GRID_H_PT);
        // Выбор выше видимой части: верх ряда у верхнего края.
        assert_eq!(scroll_to_show(400.0, 6), CELL_H_PT);
    }

    #[test]
    fn scroll_range_matches_the_grid_height() {
        // 3 ряда по 118 = 354 < 460: прокручивать нечего.
        assert_eq!(max_scroll(18), 0.0);
        assert_eq!(max_scroll(0), 0.0);
        // 10 рядов: 1180 - 460.
        assert_eq!(max_scroll(60), 10.0 * 118.0 - GRID_H_PT);
        // Неполный последний ряд считается целиком.
        assert_eq!(max_scroll(61), 11.0 * 118.0 - GRID_H_PT);
    }

    #[test]
    fn visible_range_keeps_a_row_of_margin_and_stays_in_bounds() {
        assert_eq!(visible_tiles(0.0, 200), 0..(((GRID_H_PT / CELL_H_PT).ceil() as usize + 1) * COLUMNS));
        let r = visible_tiles(118.0 * 5.0, 200);
        assert_eq!(r.start, 4 * COLUMNS);
        assert!(r.end > r.start && r.end <= 200);
        assert_eq!(visible_tiles(0.0, 4), 0..4);
        assert_eq!(visible_tiles(5000.0, 10), 10..10);
    }
}
