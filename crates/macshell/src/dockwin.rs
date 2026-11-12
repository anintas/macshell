//! Окно Dock: стеклянная подложка со значками приложений внизу экрана. Окно одно на всё время работы, во всю
//! ширину экрана; прозрачные края пропускают мышь, подложка по центру подгоняется под число значков.
//! Режимы: умный (места не занимает, виден над рабочим столом и уходит под нижний край, когда окно приложения
//! доходит до него), автоскрытие (всегда под краем, появляется, когда мышь касается края) и на виду (занимает
//! место внизу экрана, окна приложений не заходят под него).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use capture::{RectI, capture_region};
use glass::{Bitmap, GlassParams, cpu};
use slint::{Image, ModelRc, VecModel};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::anim::{ease_out_cubic, progress};
use crate::appbar::{AppBar, Edge};
use crate::bar_style::{SoftBackdrop, padded_rect};
use crate::config::DockMode;
use crate::dock::{DockItem, HEIGHT_PT, LABEL_HEIGHT_PT, MARGIN_PT, SlotRef, slots_for};
use crate::host::{InputEvent, LayeredWindow, primary_scale};
use crate::icons::{load_icon, normalize_dock_icon};
use crate::slint_view::{ContentView, DockSlot, DockView, image_from_pixels};

const ABN_POSCHANGED: usize = 1;
const ABN_FULLSCREENAPP: usize = 2;
const SLIDE_MS: u32 = 220;
const HIDE_DELAY_MS: u64 = 450;
/// Значки грузятся сразу под размер при наведении (58 pt): в покое они чуть уменьшаются, зато не размыты.
const ICON_PT: f32 = 58.0;
const EDGE_STRIP_PX: i32 = 2;
/// Внешний рендерер просит показать скрытый Dock на время перехода окна.
const MSG_MOTION_REVEAL: u32 = 0x8000 + 35;

/// Что пользователь сделал с Dock; исполняет это владелец окна.
#[derive(Clone, Debug, PartialEq)]
pub enum DockEvent {
    /// Кнопка «Приложения».
    Launcher,
    /// Клик по значку: номер в `items()`.
    Click(usize),
    /// Правый клик: номер значка и центр под ним (логические пиксели экрана).
    Menu(usize, f32),
    Downloads,
    Trash,
}

struct Slide {
    start: Instant,
    from: i32,
    to: i32,
}

pub struct DockWin {
    win: LayeredWindow,
    view: ContentView<DockView>,
    appbar: Option<AppBar>,
    strip: Option<LayeredWindow>,
    scale: f32,
    mode: DockMode,
    screen: (i32, i32),
    height_px: i32,
    label_px: i32,
    margin_px: i32,
    params: GlassParams,
    backdrop: SoftBackdrop,
    origin: (u32, u32),
    /// Какой подложке (левый край и ширина в пикселях) соответствует `glass`.
    glass: Option<(u32, u32, Bitmap)>,
    frame: Bitmap,
    items: Vec<DockItem>,
    slots: Vec<SlotRef>,
    motion_hwnds: Vec<isize>,
    icons: HashMap<String, Option<Image>>,
    clicked: Rc<Cell<i32>>,
    right: Rc<Cell<Option<(i32, f32)>>>,
    shown: bool,
    slide: Option<Slide>,
    leave_at: Option<Instant>,
    /// Повторный снимок фона после появления: закрывающееся окно ещё гаснет, когда Dock выезжает, и первый снимок
    /// ловит его. Время следующего снимка и сколько их осталось.
    resnap: Option<(Instant, u8)>,
    /// Пока открыто меню Dock, он не прячется.
    hold: bool,
    fullscreen: bool,
    /// Умный режим: окно приложения доходит до Dock, он прячется.
    overlap: bool,
    /// Кадр надо показать, даже если сам интерфейс не менялся (стекло под ним сменилось).
    dirty: bool,
}

fn dock_glass() -> GlassParams {
    GlassParams {
        radius: 28.0,
        bevel: 20.0,
        refraction: 12.0,
        blur_radius: 12,
        saturation: 1.5,
        tint: [0.05, 0.06, 0.09, 0.34],
        dispersion: 1.5,
        rim: 0.55,
        ..GlassParams::default()
    }
}

/// Сколько Dock отодвигается вниз, когда спрятан: целиком за край экрана.
/// Через сколько после появления Dock снимает фон заново, и через сколько второй раз.
const RESNAP_FIRST_MS: u64 = 350;
const RESNAP_NEXT_MS: u64 = 700;

pub fn hidden_offset(height_px: i32, margin_px: i32) -> i32 {
    height_px + margin_px
}

impl DockWin {
    pub fn open(mode: DockMode, appbar_msg: u32) -> Result<DockWin> {
        let scale = primary_scale();
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let label_px = (LABEL_HEIGHT_PT * scale).round() as i32;
        let height_px = ((HEIGHT_PT + LABEL_HEIGHT_PT) * scale).round() as i32;
        let margin_px = (MARGIN_PT * scale).round() as i32;
        let rect = RectI { x: 0, y: sh - margin_px - height_px, w: sw, h: height_px };
        let win = LayeredWindow::new(rect)?;
        win.set_title("macshell.dock");
        let view = ContentView::new_with(sw as u32, height_px as u32, DockView::new);
        view.set_scale_factor(scale);
        let clicked = Rc::new(Cell::new(-1));
        let slot = clicked.clone();
        view.ui.on_clicked(move |i| slot.set(i));
        let right = Rc::new(Cell::new(None));
        let slot = right.clone();
        view.ui.on_right_clicked(move |i, x| slot.set(Some((i, x))));

        // Умный режим и автоскрытие тоже регистрируются панелью, но без места: так приходит `ABN_FULLSCREENAPP`.
        let reserve = if mode == DockMode::Fixed { height_px - label_px + margin_px } else { 0 };
        let appbar = AppBar::register_at(win.hwnd(), appbar_msg, Edge::Bottom, reserve).ok();

        let params = dock_glass().scaled(scale);
        let band = RectI { x: 0, y: rect.y, w: sw, h: height_px };
        let padded = padded_rect(band, 2 * params.blur_radius as i32, sw, sh);
        let backdrop = SoftBackdrop::new(&capture_region(padded)?, params.blur_radius);
        let origin = ((band.x - padded.x) as u32, (band.y - padded.y) as u32);

        let strip = if mode != DockMode::Fixed { edge_strip(sw, sh).ok() } else { None };
        let mut dock = DockWin {
            win,
            view,
            appbar,
            strip,
            scale,
            mode,
            screen: (sw, sh),
            height_px,
            label_px,
            margin_px,
            params,
            backdrop,
            origin,
            glass: None,
            frame: Bitmap::new(sw as u32, height_px as u32),
            items: Vec::new(),
            slots: Vec::new(),
            motion_hwnds: Vec::new(),
            icons: HashMap::new(),
            clicked,
            right,
            shown: mode != DockMode::Autohide,
            slide: None,
            leave_at: None,
            resnap: None,
            hold: false,
            fullscreen: false,
            overlap: false,
            dirty: true,
        };
        dock.place();
        if !dock.shown {
            dock.win.set_blend(255, hidden_offset(height_px, margin_px))?;
        }
        dock.set_items(Vec::new());
        dock.present()?;
        Ok(dock)
    }

    pub fn items(&self) -> &[DockItem] {
        &self.items
    }

    /// Верхний край окна Dock на экране (пиксели): над ним встают меню значков.
    pub fn top_px(&self) -> i32 {
        self.win.rect().y + self.label_px
    }

    pub fn mode(&self) -> DockMode {
        self.mode
    }

    /// Занятая полоса внизу экрана (пиксели): нужна только в режиме «на виду».
    fn reserve_px(&self) -> i32 {
        if self.mode == DockMode::Fixed { self.height_px - self.label_px + self.margin_px } else { 0 }
    }

    /// Dock сейчас прячется под нижний край, когда мышь ушла.
    fn autohides(&self) -> bool {
        self.mode == DockMode::Autohide || (self.mode == DockMode::Smart && self.overlap)
    }

    /// Где стоит подложка на экране (пиксели): по ней решается, задевает ли её окно приложения.
    pub fn pill_rect(&self) -> RectI {
        let (x, w) = self.pill();
        let r = self.win.rect();
        RectI { x: x as i32, y: r.y + self.label_px, w: w as i32, h: self.height_px - self.label_px + self.margin_px }
    }

    /// Умный режим: окно приложения дошло до Dock (или ушло). В остальных режимах ничего не меняет.
    pub fn set_overlap(&mut self, overlap: bool) {
        if self.mode != DockMode::Smart || self.overlap == overlap {
            return;
        }
        self.overlap = overlap;
        if overlap {
            // Не мгновенно: окно, проезжающее мимо при перетаскивании, не должно мигать Dock.
            if self.shown {
                self.leave_at = Some(Instant::now() + Duration::from_millis(HIDE_DELAY_MS));
            }
        } else {
            self.leave_at = None;
            if !self.shown && !self.fullscreen {
                self.reveal();
            }
        }
    }

    /// Верх окна: над занятой полосой или над нижним краем экрана.
    fn place(&mut self) {
        let bottom = match &self.appbar {
            Some(bar) if self.mode == DockMode::Fixed && bar.rect().h > 0 => bar.rect().y + bar.rect().h,
            _ => self.screen.1,
        };
        let _ = self.win.set_position(0, bottom - self.margin_px - self.height_px);
        if !self.items.is_empty() { self.publish_motion_targets(); }
    }

    /// Новый набор значков; окно перерисуется при ближайшем `present`.
    pub fn set_items(&mut self, items: Vec<DockItem>) {
        let px = (ICON_PT * self.scale).round() as i32;
        self.slots = slots_for(&items);
        self.slots.extend([SlotRef::Separator, SlotRef::Downloads, SlotRef::Trash]);
        self.view.ui.set_fit(crate::dock::fit_factor(&self.slots, self.screen.0 as f32 / self.scale));
        let model: Vec<DockSlot> = self
            .slots
            .iter()
            .map(|s| match *s {
                SlotRef::Launcher => DockSlot { icon: Image::default(), has_icon: false, running: false, kind: 1, name: "Приложения".into() },
                SlotRef::Separator => DockSlot { icon: Image::default(), has_icon: false, running: false, kind: 2, name: "".into() },
                SlotRef::Downloads => DockSlot { icon: Image::default(), has_icon: false, running: false, kind: 3, name: "Загрузки".into() },
                SlotRef::Trash => DockSlot { icon: Image::default(), has_icon: false, running: false, kind: 4, name: "Корзина".into() },
                SlotRef::Item(i) => {
                    let item = &items[i];
                    let icon = self
                        .icons
                        .entry(item.target.clone())
                        .or_insert_with(|| load_icon(&item.target, px).map(|p| image_from_pixels(&normalize_dock_icon(&p))))
                        .clone();
                    DockSlot { has_icon: icon.is_some(), icon: icon.unwrap_or_default(), running: item.running(), kind: 0, name: item.name.as_str().into() }
                }
            })
            .collect();
        self.view.ui.set_slots(ModelRc::new(VecModel::from(model)));
        self.items = items;
        self.publish_motion_targets();
    }

    /// Неблокирующий обмен с рендерером анимации: только координаты окон, без данных приложений.
    fn publish_motion_targets(&mut self) {
        use windows::Win32::Foundation::{HANDLE, HWND};
        use windows::Win32::UI::WindowsAndMessaging::{RemovePropW, SetPropW};
        use windows::core::PCWSTR;

        let dock = HWND(self.win.hwnd() as *mut _);
        let name = |axis: &str, hwnd: isize| -> Vec<u16> {
            format!("macshell.motion.{axis}.{:016X}", hwnd as usize as u64).encode_utf16().chain(Some(0)).collect()
        };
        // Update existing properties in place: the animation hooks run in other
        // processes and can read the targets while the Dock is being refreshed.
        // Removing every property first made otherwise valid animations fall back.
        let previous_hwnds = std::mem::take(&mut self.motion_hwnds);
        let screen_pt = self.screen.0 as f32 / self.scale;
        let fit = crate::dock::fit_factor(&self.slots, screen_pt);
        let hover = self.view.ui.get_hover_index();
        for (index, item) in self.items.iter().enumerate() {
            let Some((x, _)) = crate::dock::motion_target(&self.slots, index, screen_pt, self.scale) else { continue };
            let Some(slot_index) = self.slots.iter().position(|slot| *slot == SlotRef::Item(index)) else { continue };
            let distance = if hover < 0 { i32::MAX } else { (hover - slot_index as i32).abs() };
            let icon_pt = if distance == 0 { 58.0 } else if distance == 1 { 53.0 } else { 48.0 };
            let size = icon_pt * fit * self.scale;
            let y = self.win.rect().y as f32 + (LABEL_HEIGHT_PT + HEIGHT_PT - (16.0 + icon_pt / 2.0) * fit) * self.scale;
            if x < 0.0 || y < 0.0 || size < 1.0 { continue; }
            for &hwnd in &item.hwnds {
                let mut ok = true;
                for (axis, value) in [("x", x), ("y", y), ("size", size), ("origin", self.win.rect().y as f32)] {
                    let key = name(axis, hwnd);
                    let raw = value.round() as usize + 1;
                    ok &= unsafe { SetPropW(dock, PCWSTR(key.as_ptr()), Some(HANDLE(raw as *mut _))) }.is_ok();
                }
                if ok || previous_hwnds.contains(&hwnd) {
                    if !self.motion_hwnds.contains(&hwnd) { self.motion_hwnds.push(hwnd); }
                } else {
                    for axis in ["x", "y", "size", "origin"] {
                        let key = name(axis, hwnd);
                        unsafe { let _ = RemovePropW(dock, PCWSTR(key.as_ptr())); }
                    }
                }
            }
        }
        for hwnd in previous_hwnds {
            if self.motion_hwnds.contains(&hwnd) { continue; }
            for axis in ["x", "y", "size", "origin"] {
                let key = name(axis, hwnd);
                unsafe { let _ = RemovePropW(dock, PCWSTR(key.as_ptr())); }
            }
        }
    }

    /// Фон снимается заново: сменились обои или Dock показывается над окнами (автоскрытие).
    pub fn recapture(&mut self) -> Result<()> {
        let (sw, sh) = self.screen;
        let band = RectI { x: 0, y: self.win.rect().y, w: sw, h: self.height_px };
        let padded = padded_rect(band, 2 * self.params.blur_radius as i32, sw, sh);
        self.backdrop = SoftBackdrop::new(&capture_region(padded)?, self.params.blur_radius);
        self.origin = ((band.x - padded.x) as u32, (band.y - padded.y) as u32);
        self.glass = None;
        self.dirty = true;
        Ok(())
    }

    fn pill(&self) -> (u32, u32) {
        let x = (self.view.ui.get_pill_x() * self.scale).round().max(0.0) as u32;
        let w = (self.view.ui.get_pill_width() * self.scale).round().max(1.0) as u32;
        (x, w.min(self.frame.width.saturating_sub(x)))
    }

    /// Мышь, сообщения панели и автоскрытие. Возвращает действия пользователя.
    pub fn handle_input(&mut self) -> Vec<DockEvent> {
        let mut out = Vec::new();
        let mut reveal = false;
        let previous_hover = self.view.ui.get_hover_index();
        for ev in self.win.take_input() {
            match ev {
                InputEvent::Message { msg: MSG_MOTION_REVEAL, .. } => {
                    if !self.shown && !self.fullscreen { self.reveal(); }
                }
                InputEvent::Message { wparam: ABN_FULLSCREENAPP, lparam, .. } => self.set_fullscreen(lparam != 0),
                InputEvent::Message { wparam: ABN_POSCHANGED, .. } => {
                    let reserve = self.reserve_px();
                    if self.appbar.as_mut().is_some_and(|b| b.reposition(reserve).unwrap_or(false)) {
                        self.place();
                    }
                }
                InputEvent::Leave => {
                    self.view.ui.set_hover_index(-1);
                    if self.autohides() && self.shown {
                        self.leave_at = Some(Instant::now() + Duration::from_millis(HIDE_DELAY_MS));
                    }
                    self.view.dispatch(&ev);
                }
                InputEvent::Move { .. } => {
                    self.leave_at = None;
                    self.view.dispatch(&ev);
                }
                other => self.view.dispatch(&other),
            }
        }
        if self.view.ui.get_hover_index() != previous_hover { self.publish_motion_targets(); }
        if let Some(strip) = &self.strip {
            for ev in strip.take_input() {
                if matches!(ev, InputEvent::Move { .. } | InputEvent::Down { .. }) {
                    reveal = true;
                }
            }
        }
        if reveal && !self.shown && !self.fullscreen {
            self.reveal();
        }
        let i = self.clicked.replace(-1);
        if i >= 0 {
            match self.slots.get(i as usize) {
                Some(SlotRef::Launcher) => out.push(DockEvent::Launcher),
                Some(SlotRef::Item(n)) => out.push(DockEvent::Click(*n)),
                Some(SlotRef::Downloads) => out.push(DockEvent::Downloads),
                Some(SlotRef::Trash) => out.push(DockEvent::Trash),
                _ => {}
            }
        }
        if let Some((i, x)) = self.right.take() {
            if let Some(SlotRef::Item(n)) = self.slots.get(i as usize) {
                out.push(DockEvent::Menu(*n, x));
            }
        }
        out
    }

    fn reveal(&mut self) {
        let _ = self.recapture();
        self.resnap = Some((Instant::now() + Duration::from_millis(RESNAP_FIRST_MS), 2));
        self.shown = true;
        self.leave_at = self.autohides().then(|| Instant::now() + Duration::from_millis(HIDE_DELAY_MS * 4));
        if crate::anim::enabled() {
            self.slide = Some(Slide { start: Instant::now(), from: self.win.blend().1, to: 0 });
        } else {
            self.slide = None;
            let _ = self.win.set_blend(255, 0);
        }
    }

    fn conceal(&mut self) {
        self.shown = false;
        self.leave_at = None;
        let to = hidden_offset(self.height_px, self.margin_px);
        if crate::anim::enabled() {
            self.slide = Some(Slide { start: Instant::now(), from: self.win.blend().1, to });
        } else {
            self.slide = None;
            let _ = self.win.set_blend(255, to);
        }
    }

    /// Окна на экране поменялись (открылось или закрылось): фон под Dock снимается заново, когда они улягутся.
    pub fn resnap_soon(&mut self) {
        if self.shown {
            self.resnap = Some((Instant::now() + Duration::from_millis(RESNAP_FIRST_MS), 2));
        }
    }

    /// Показывает спрятанный Dock (Win открывает Dock вместе с сеткой приложений).
    pub fn show(&mut self) {
        if !self.shown && !self.fullscreen {
            self.reveal();
        }
    }

    /// Пока меню Dock открыто, Dock не прячется.
    pub fn set_hold(&mut self, hold: bool) {
        if self.hold == hold {
            return;
        }
        self.hold = hold;
        if !hold && self.autohides() && self.shown {
            self.leave_at = Some(Instant::now() + Duration::from_millis(HIDE_DELAY_MS));
        }
    }

    fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
        if let Some(bar) = &self.appbar {
            bar.set_fullscreen(on);
        }
        if let Some(strip) = &self.strip {
            strip.set_topmost(!on);
        }
    }

    /// Подложка и значки поверх стекла нужной ширины.
    pub fn present(&mut self) -> Result<()> {
        let content = match self.view.render_if_changed() {
            Some(c) => c,
            None if self.dirty => self.view.render(),
            None => return Ok(()),
        };
        self.dirty = false;
        let (px, pw) = self.pill();
        if self.glass.as_ref().map(|(x, w, _)| (*x, *w)) != Some((px, pw)) {
            let inner = self.backdrop.region(self.origin.0 + px, self.origin.1 + self.label_px as u32, pw, self.frame.height - self.label_px as u32);
            self.glass = Some((px, pw, cpu::render(&inner, &GlassParams { blur_radius: 0, ..self.params })));
        }
        self.frame.pixels.fill(0);
        if let Some((x, w, glass)) = &self.glass {
            let stride = self.frame.width as usize;
            for row in 0..glass.height as usize {
                let src = &glass.pixels[row * *w as usize..(row + 1) * *w as usize];
                let at = (row + self.label_px as usize) * stride + *x as usize;
                self.frame.pixels[at..at + src.len()].copy_from_slice(src);
            }
        }
        cpu::composite_over(&mut self.frame, &content);
        self.win.present(&self.frame)
    }

    /// Скольжение и таймер скрытия. `true`, пока нужен кадр.
    pub fn tick(&mut self) -> Result<bool> {
        if let Some((at, left)) = self.resnap {
            if Instant::now() >= at {
                let _ = self.recapture();
                self.resnap = (left > 1).then(|| (Instant::now() + Duration::from_millis(RESNAP_NEXT_MS), left - 1));
            }
        }
        if let Some(at) = self.leave_at {
            if Instant::now() >= at && !self.hold && self.shown {
                self.conceal();
            }
        }
        let mut animating = self.view.animating();
        if let Some(slide) = &self.slide {
            let t = ease_out_cubic(progress(slide.start, SLIDE_MS));
            let dy = slide.from + ((slide.to - slide.from) as f32 * t).round() as i32;
            self.win.set_blend(255, dy)?;
            if t >= 1.0 {
                self.slide = None;
            } else {
                animating = true;
            }
        }
        Ok(animating)
    }

    /// Через сколько миллисекунд нужен следующий шаг без событий (таймер скрытия).
    pub fn next_wake_ms(&self) -> u32 {
        let hide = match self.leave_at {
            Some(at) if self.shown && !self.hold => at.saturating_duration_since(Instant::now()).as_millis() as u32 + 1,
            _ => u32::MAX,
        };
        let resnap = self.resnap.map_or(u32::MAX, |(at, _)| at.saturating_duration_since(Instant::now()).as_millis() as u32 + 1);
        hide.min(resnap)
    }
}

/// Невидимая полоска вдоль нижнего края: ловит мышь, когда Dock спрятан. Один процент прозрачности, чтобы
/// окно получало мышь (полностью прозрачные точки мышь пропускают).
fn edge_strip(width: i32, screen_h: i32) -> Result<LayeredWindow> {
    let strip = LayeredWindow::new(RectI { x: 0, y: screen_h - EDGE_STRIP_PX, w: width, h: EDGE_STRIP_PX })?;
    let mut bmp = Bitmap::new(width as u32, EDGE_STRIP_PX as u32);
    bmp.pixels.fill(0x0100_0000);
    strip.present(&bmp)?;
    Ok(strip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_dock_sits_entirely_below_the_screen_edge() {
        // Окно на 8 px выше края; сдвиг на высоту + поле выводит его за край.
        assert_eq!(hidden_offset(72, 8), 80);
    }
}
