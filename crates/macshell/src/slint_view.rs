//! Содержимое окна: Slint с программным рендером в предумноженный BGRA.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::platform::PointerEventButton;
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, PlatformError};

use crate::host::InputEvent;

slint::include_modules!();

/// Цвет системы (`accent` в конфиге) для новых окон; по умолчанию синий.
static ACCENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(DEFAULT_ACCENT);
pub const DEFAULT_ACCENT: u32 = 0x4673E2;

/// Ставит цвет системы: действует на окна, созданные после этого вызова.
pub fn set_accent(rgb: [u8; 3]) {
    ACCENT.store(u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]), std::sync::atomic::Ordering::Relaxed);
}

pub fn accent() -> [u8; 3] {
    let v = ACCENT.load(std::sync::atomic::Ordering::Relaxed).to_be_bytes();
    [v[1], v[2], v[3]]
}

/// Окно, которому можно выдать цвет системы (глобальная `Palette` у каждого свой).
pub trait Themed {
    fn apply_accent(&self, rgb: [u8; 3]);
}

macro_rules! themed {
    ($($t:ty),* $(,)?) => {
        $(impl Themed for $t {
            fn apply_accent(&self, rgb: [u8; 3]) {
                self.global::<Palette>().set_accent(slint::Color::from_rgb_u8(rgb[0], rgb[1], rgb[2]));
                self.global::<Palette>().set_animations_enabled(crate::anim::enabled());
            }
        })*
    };
}

themed!(
    Proto, TopBar, Menu, TrayFlyout, Spotlight, VolumePopup, Apps, DockView, CalendarPopup, ControlCenter, Switcher, NotesWidget,
    ClockWidget, SystemWidget, WeatherWidget, NowPlayingWidget, StorageWidget, CurrencyWidget, WidgetGuide, Settings, MissionControl, StageStrip, CaptionBackground, CaptionLights,
);

thread_local! {
    /// Окно, которое платформа создала последним; `ContentView::new` забирает его.
    static NEW_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

/// Картинка Slint из пикселей значка.
pub fn image_from_pixels(icon: &crate::icons::IconPixels) -> slint::Image {
    slint::Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &icon.rgba,
        icon.width,
        icon.height,
    ))
}

struct HostPlatform;

impl Platform for HostPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        NEW_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
        Ok(window)
    }
}

pub struct ContentView<C: ComponentHandle> {
    pub ui: C,
    window: Rc<MinimalSoftwareWindow>,
    size: (usize, usize),
    scale: Cell<f32>,
    /// Буфер Slint живёт между кадрами: при `ReusedBuffer` перерисовываются только изменившиеся места.
    buf: RefCell<Vec<PremultipliedRgbaColor>>,
    cache: RefCell<Vec<u32>>,
}

impl ContentView<Proto> {
    /// Прототипная панель Spotlight.
    pub fn new(width: u32, height: u32) -> ContentView<Proto> {
        ContentView::new_with(width, height, Proto::new)
    }
}

impl<C: ComponentHandle> ContentView<C> {
    /// Окно Slint размером `width` x `height` физических пикселей с компонентом из `ctor`.
    pub fn new_with(width: u32, height: u32, ctor: impl FnOnce() -> Result<C, PlatformError>) -> ContentView<C>
    where
        C: Themed,
    {
        // Повторная установка платформы вернёт ошибку: она уже стоит, так и надо.
        let _ = slint::platform::set_platform(Box::new(HostPlatform));
        let ui = ctor().expect("create component");
        ui.apply_accent(accent());
        let window = NEW_WINDOW
            .with(|slot| slot.borrow_mut().take())
            .expect("platform did not create a window");
        window.set_size(PhysicalSize::new(width, height));
        ui.show().expect("show component");
        let size = (width as usize, height as usize);
        let buf = RefCell::new(vec![PremultipliedRgbaColor { red: 0, green: 0, blue: 0, alpha: 0 }; size.0 * size.1]);
        ContentView { ui, window, size, scale: Cell::new(1.0), buf, cache: RefCell::new(vec![0; size.0 * size.1]) }
    }

    /// Масштаб DPI (1.25 для 125%): размеры в `.slint` умножаются на него.
    pub fn set_scale_factor(&self, scale: f32) {
        self.scale.set(scale);
        self.ui.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
        // ScaleFactorChanged сохраняет логический размер и меняет физический. Хост уже выделил буфер
        // под физические размеры окна Windows, поэтому после смены DPI возвращаем их явно.
        self.window.set_size(PhysicalSize::new(self.size.0 as u32, self.size.1 as u32));
    }

    /// Передаёт событие мыши Slint. Позиции приходят в физических пикселях окна.
    pub fn dispatch(&self, ev: &InputEvent) {
        let s = self.scale.get();
        let at = |x: i32, y: i32| LogicalPosition::new(x as f32 / s, y as f32 / s);
        let event = match *ev {
            InputEvent::Move { x, y } => WindowEvent::PointerMoved { position: at(x, y) },
            InputEvent::Down { x, y } => {
                WindowEvent::PointerPressed { position: at(x, y), button: PointerEventButton::Left }
            }
            InputEvent::Up { x, y } => {
                WindowEvent::PointerReleased { position: at(x, y), button: PointerEventButton::Left }
            }
            InputEvent::RightUp { x, y } => {
                // Правая кнопка приходит одним событием: нажатие и отпускание подряд.
                let position = at(x, y);
                self.ui.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Right });
                WindowEvent::PointerReleased { position, button: PointerEventButton::Right }
            }
            InputEvent::Leave => WindowEvent::PointerExited,
            // Клавиатуру разбирает Rust-код окна (поле ввода Spotlight), в Slint она не нужна.
            InputEvent::Message { .. } | InputEvent::Char(_) | InputEvent::Key { .. } | InputEvent::Wheel { .. } => return,
        };
        self.ui.window().dispatch_event(event);
    }

    /// Идёт ли анимация Slint (подсветка под курсором и т.п.): пока да, цикл должен рисовать кадры каждые 16 мс.
    pub fn animating(&self) -> bool {
        self.window.has_active_animations()
    }

    /// Рисует, если интерфейс изменился: `Some(кадр)` в предумноженном BGRA `0xAARRGGBB`, иначе `None`.
    pub fn render_if_changed(&self) -> Option<Vec<u32>> {
        slint::platform::update_timers_and_animations();
        let w = self.size.0;
        // В покое кадр не рисуется вовсе. Иначе Slint перерисовывает только изменившиеся места (подсветка под
        // курсором, часы) в буфере прошлого кадра, а не весь кадр в новом буфере.
        let mut buf = self.buf.borrow_mut();
        let drew = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut buf, w);
        });
        if !drew {
            return None;
        }
        let mut cache = self.cache.borrow_mut();
        for (out, p) in cache.iter_mut().zip(buf.iter()) {
            *out = (p.alpha as u32) << 24 | (p.red as u32) << 16 | (p.green as u32) << 8 | p.blue as u32;
        }
        Some(cache.clone())
    }

    /// Кадр; без изменений в интерфейсе возвращает прошлый.
    pub fn render(&self) -> Vec<u32> {
        self.render_if_changed();
        self.cache.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_renders_nonempty() {
        let px = ContentView::new(300, 60).render();
        assert_eq!(px.len(), 18000);
        assert!(px.iter().any(|p| p >> 24 != 0));
    }

    #[test]
    fn topbar_renders_nonempty() {
        let view = ContentView::new_with(800, 28, TopBar::new);
        view.ui.set_clock("Пн 2 нояб. 20:31".into());
        let px = view.render();
        let right_third_start = 800 * 2 / 3;
        let visible = (0..28usize).any(|y| (right_third_start..800).any(|x| px[y * 800 + x] >> 24 != 0));
        assert!(visible, "clock text must paint pixels in the right third");
    }

    #[test]
    fn scaled_render_keeps_the_physical_buffer_size() {
        let view = ContentView::new_with(350, 400, CalendarPopup::new);
        view.set_scale_factor(1.25);
        assert_eq!(view.render().len(), 350 * 400);
    }

    #[test]
    fn dock_hover_shows_title_and_system_slots_remain_clickable() {
        use std::cell::Cell;
        use std::rc::Rc;
        use slint::{ModelRc, VecModel};
        let view = ContentView::new_with(500, 102, DockView::new);
        view.ui.global::<Palette>().set_animations_enabled(false);
        let slot = |kind, name: &str| DockSlot { icon: Default::default(), has_icon: false, running: false, kind, name: name.into() };
        view.ui.set_slots(ModelRc::new(VecModel::from(vec![slot(1, "Приложения"), slot(2, ""), slot(3, "Загрузки"), slot(4, "Корзина")])));
        let selected = Rc::new(Cell::new(-1));
        let result = selected.clone();
        view.ui.on_clicked(move |i| result.set(i));
        view.render();
        let x = (view.ui.get_pill_x() + 10.0 + 60.0 + 14.0 + 30.0) as i32;
        view.dispatch(&InputEvent::Move { x, y: 65 });
        assert_eq!(view.ui.get_hover_index(), 2);
        let pixels = view.render();
        assert!((0..30usize).any(|y| (0..500usize).any(|x| pixels[y * 500 + x] >> 24 > 0)), "название над Dock должно рисоваться");
        click(&view, x, 65);
        assert_eq!(selected.get(), 2);
    }

    #[test]
    fn dock_title_fades_in_when_motion_is_enabled() {
        use slint::{ModelRc, VecModel};
        use std::time::Duration;
        let view = ContentView::new_with(500, 102, DockView::new);
        view.ui.global::<Palette>().set_animations_enabled(true);
        view.ui.set_slots(ModelRc::new(VecModel::from(vec![DockSlot {
            icon: Default::default(), has_icon: false, running: false, kind: 1, name: "Приложения".into(),
        }])));
        view.render();
        view.ui.set_hover_index(0);
        let first = view.render();
        std::thread::sleep(Duration::from_millis(140));
        let last = view.render();
        let alpha = |pixels: &[u32]| -> u64 { pixels.iter().take(500 * 28).map(|p| (p >> 24) as u64).sum() };
        assert!(alpha(&last) > alpha(&first), "название должно плавно появляться");
    }

    #[test]
    #[ignore = "writes preview frames"]
    fn dump_dock_motion_frames() {
        use slint::{ModelRc, VecModel};
        use std::time::Duration;
        let dir = std::env::var("MACSHELL_DUMP_DIR").expect("MACSHELL_DUMP_DIR");
        let view = ContentView::new_with(500, 102, DockView::new);
        view.ui.global::<Palette>().set_animations_enabled(true);
        let slot = |name: &str| DockSlot { icon: Default::default(), has_icon: false, running: false, kind: 1, name: name.into() };
        view.ui.set_slots(ModelRc::new(VecModel::from(vec![slot("Приложения"), slot("Поиск"), slot("Загрузки")])));
        view.render();
        view.ui.set_hover_index(1);
        for (i, delay) in [0, 55, 100].into_iter().enumerate() {
            std::thread::sleep(Duration::from_millis(delay));
            let bmp = glass::Bitmap { width: 500, height: 102, pixels: view.render() };
            crate::snapshot::write_bmp(&format!("{dir}\\dock_motion_{i}.bmp"), &bmp).unwrap();
        }
    }

    #[test]
    fn dock_fits_a_320_point_screen_at_125_percent() {
        use slint::{ModelRc, VecModel};
        use crate::dock::SlotRef;
        let view = ContentView::new_with(400, 128, DockView::new);
        view.set_scale_factor(1.25);
        let slots = vec![SlotRef::Launcher, SlotRef::Separator, SlotRef::Item(0), SlotRef::Item(1), SlotRef::Item(2), SlotRef::Item(3), SlotRef::Separator, SlotRef::Downloads, SlotRef::Trash];
        let model: Vec<DockSlot> = slots.iter().map(|slot| DockSlot { icon: Default::default(), has_icon: false, running: matches!(slot, SlotRef::Item(_)), kind: match slot { SlotRef::Launcher => 1, SlotRef::Separator => 2, SlotRef::Downloads => 3, SlotRef::Trash => 4, _ => 0 }, name: "Приложение".into() }).collect();
        view.ui.set_slots(ModelRc::new(VecModel::from(model)));
        view.ui.set_fit(crate::dock::fit_factor(&slots, 320.0));
        view.render();
        let left = view.ui.get_pill_x();
        let width = view.ui.get_pill_width();
        assert!(left >= 0.0 && left + width <= 320.01, "Dock extends outside: {left} + {width}");
    }

    /// Клик в точке (x, y) физических пикселей: движение, нажатие, отпускание.
    fn click<C: ComponentHandle>(view: &ContentView<C>, x: i32, y: i32) {
        use crate::host::InputEvent;
        for ev in [InputEvent::Move { x, y }, InputEvent::Down { x, y }, InputEvent::Up { x, y }] {
            view.dispatch(&ev);
        }
    }


    /// Середина строки с пунктом `id` в окне меню (логические пиксели, масштаб 1).
    fn row_center(entries: &[MenuEntry], id: i32) -> i32 {
        let mut y = crate::menu::PADDING / 2.0;
        for e in entries {
            let h = if e.separator { crate::menu::SEPARATOR } else { crate::menu::ROW };
            if e.id == id && !e.separator {
                return (y + h / 2.0) as i32;
            }
            y += h;
        }
        panic!("no entry {id}");
    }

    #[test]
    fn menu_click_activates_enabled_rows_only() {
        use crate::menu::{ID_FORCE_QUIT, ID_QUIT, ID_RESTART, height, logo_menu};
        use slint::{ModelRc, VecModel};
        use std::cell::Cell;
        use std::rc::Rc;

        let entries = logo_menu(false, false, false);
        let view = ContentView::new_with(270, height(&entries).ceil() as u32, Menu::new);
        view.ui.set_entries(ModelRc::new(VecModel::from(entries.clone())));
        let got = Rc::new(Cell::new(-1));
        let slot = got.clone();
        view.ui.on_activated(move |i| slot.set(i));
        view.render();
        click(&view, 100, row_center(&entries, ID_FORCE_QUIT)); // серый
        assert_eq!(got.get(), -1);
        click(&view, 100, row_center(&entries, ID_QUIT));
        assert_eq!(got.get(), ID_QUIT);
        click(&view, 100, row_center(&entries, ID_RESTART));
        assert_eq!(got.get(), ID_RESTART);
    }

    #[test]
    fn window_layout_menu_emits_selected_layout() {
        use crate::menu::{height, window_layout_menu, ID_WINDOW_LAYOUT_FIRST};
        use slint::{ModelRc, VecModel};
        let entries = window_layout_menu();
        let view = ContentView::new_with(270, height(&entries).ceil() as u32, Menu::new);
        view.ui.set_entries(ModelRc::new(VecModel::from(entries.clone())));
        let got = Rc::new(Cell::new(-1));
        let slot = got.clone();
        view.ui.on_activated(move |i| slot.set(i));
        let pixels = view.render();
        if let Ok(path) = std::env::var("MACSHELL_LAYOUT_PREVIEW") {
            let h = height(&entries).ceil() as u32;
            let mut background = glass::Bitmap { width: 270, height: h, pixels: vec![0xff30333a; 270 * h as usize] };
            glass::cpu::composite_over(&mut background, &pixels);
            crate::snapshot::write_bmp(&path, &background).unwrap();
        }
        click(&view, 100, row_center(&entries, ID_WINDOW_LAYOUT_FIRST + 2));
        assert_eq!(got.get(), ID_WINDOW_LAYOUT_FIRST + 2);
    }

    #[test]
    fn caption_lights_emit_distinct_actions() {
        let view = ContentView::new_with(78, 30, CaptionLights::new);
        let got = Rc::new(Cell::new(-1));
        let slot = got.clone();
        view.ui.on_activated(move |i| slot.set(i));
        view.render();
        for (x, expected) in [(13, 0), (39, 1), (65, 2)] {
            click(&view, x, 15);
            assert_eq!(got.get(), expected);
        }
    }

    #[test]
    fn pins_report_left_and_right_clicks() {
        use crate::host::InputEvent;
        use slint::{ModelRc, VecModel};
        use std::cell::RefCell;
        use std::collections::BTreeSet;
        use std::rc::Rc;

        let view = ContentView::new_with(800, 28, TopBar::new);
        let pins: Vec<PinItem> =
            (0..2).map(|i| PinItem { icon: Default::default(), name: format!("app{i}").into() }).collect();
        view.ui.set_pins(ModelRc::new(VecModel::from(pins)));
        let left = Rc::new(RefCell::new(BTreeSet::new()));
        let right = Rc::new(RefCell::new(BTreeSet::new()));
        let l = left.clone();
        view.ui.on_pin_clicked(move |i| {
            l.borrow_mut().insert(i);
        });
        let r = right.clone();
        view.ui.on_pin_menu(move |i, _| {
            r.borrow_mut().insert(i);
        });
        view.render();
        // Точное положение значков зависит от ширины часов: проходим правую половину полосы.
        for x in (400..800).step_by(2) {
            click(&view, x, 14);
            view.dispatch(&InputEvent::RightUp { x, y: 14 });
        }
        assert_eq!(*left.borrow(), BTreeSet::from([0, 1]));
        assert_eq!(*right.borrow(), BTreeSet::from([0, 1]));
    }

    #[test]
    fn flyout_reports_left_right_clicks_and_pin_toggle() {
        use crate::host::InputEvent;
        use crate::tray::{flyout_size, tile_center};
        use slint::{ModelRc, VecModel};
        use std::cell::RefCell;
        use std::rc::Rc;

        let n = 5;
        let (w, h) = flyout_size(n);
        let view = ContentView::new_with(w as u32, h as u32, TrayFlyout::new);
        let items: Vec<TrayItem> = (0..n)
            .map(|i| TrayItem { limited: false, name: format!("App {i}").into(), icon: Default::default(), tip: format!("app{i}").into(), pinned: i == 1, hidden: false })
            .collect();
        view.ui.set_items(ModelRc::new(VecModel::from(items)));
        let log = Rc::new(RefCell::new(Vec::<String>::new()));
        let l = log.clone();
        view.ui.on_clicked(move |i| l.borrow_mut().push(format!("L{i}")));
        let l = log.clone();
        view.ui.on_right_clicked(move |i| l.borrow_mut().push(format!("R{i}")));
        let l = log.clone();
        view.ui.on_pin_toggled(move |i| l.borrow_mut().push(format!("P{i}")));
        view.render();

        let (x, y) = tile_center(4);
        click(&view, x as i32, y as i32);
        view.dispatch(&InputEvent::RightUp { x: x as i32, y: y as i32 });
        let (x, y) = tile_center(2);
        view.dispatch(&InputEvent::Move { x: x as i32, y: y as i32 }); // навели на плитку: булавка видна
        click(&view, x as i32 + 11, y as i32 - 11); // булавка в правом верхнем углу плитки
        assert_eq!(*log.borrow(), vec!["L4", "R4", "P2"]);

        let (x, y) = tile_center(3);
        view.dispatch(&InputEvent::Move { x: x as i32, y: y as i32 });
        assert_eq!(view.ui.get_hover_tip(), "app3");
    }

    #[test]
    fn partial_repaint_matches_a_full_repaint() {
        use crate::host::InputEvent;

        // Кадр после нескольких частичных перерисовок совпадает с кадром, нарисованным с нуля.
        let view = ContentView::new_with(800, 28, TopBar::new);
        view.ui.set_app_name("Microsoft Edge".into());
        view.ui.set_clock("Пн 2 нояб. 20:31".into());
        view.render();
        view.ui.set_app_name("Claude".into()); // короче прежнего: хвост старого текста должен стереться
        view.ui.set_clock("Пн 2 нояб. 20:32".into());
        view.dispatch(&InputEvent::Move { x: 20, y: 14 }); // подсветка логотипа
        view.render();
        view.dispatch(&InputEvent::Leave);
        std::thread::sleep(std::time::Duration::from_millis(200)); // подсветка гаснет за 120 мс
        view.render();
        let partial = view.render();

        let fresh = ContentView::new_with(800, 28, TopBar::new);
        fresh.ui.set_app_name("Claude".into());
        fresh.ui.set_clock("Пн 2 нояб. 20:32".into());
        let full = fresh.render();
        let differ = partial.iter().zip(&full).filter(|(a, b)| a != b).count();
        assert_eq!(differ, 0, "{differ} pixels differ between partial and full repaint");
    }

    #[test]
    fn tile_without_icon_shows_a_placeholder() {
        use crate::tray::{flyout_size, tile_center};
        use slint::{ModelRc, VecModel};

        let (w, h) = flyout_size(1);
        let view = ContentView::new_with(w as u32, h as u32, TrayFlyout::new);
        let item = TrayItem { limited: false, name: "App".into(), icon: Default::default(), tip: "".into(), pinned: false, hidden: false };
        view.ui.set_items(ModelRc::new(VecModel::from(vec![item])));
        let px = view.render();
        // Курсора нет: подсветки плитки нет, значит видимые пиксели в середине плитки рисует только заглушка.
        let (cx, cy) = tile_center(0);
        let painted = (cy as usize - 10..cy as usize + 10)
            .flat_map(|y| (cx as usize - 10..cx as usize + 10).map(move |x| (x, y)))
            .filter(|&(x, y)| px[y * w as usize + x] >> 24 > 60)
            .count();
        assert!(painted > 20, "tile with no icon must not be empty, painted {painted}");
    }

    #[test]
    fn topbar_tray_chevron_and_pins_report_clicks() {
        use crate::host::InputEvent;
        use slint::{ModelRc, VecModel};
        use std::cell::{Cell, RefCell};
        use std::collections::BTreeSet;
        use std::rc::Rc;

        let view = ContentView::new_with(800, 28, TopBar::new);
        view.ui.set_status_order(ModelRc::new(VecModel::from(vec![6, 7, 8, 9])));
        let pins: Vec<TrayItem> =
            (0..2).map(|i| TrayItem { limited: false, name: format!("T {i}").into(), icon: Default::default(), tip: format!("t{i}").into(), pinned: true, hidden: false }).collect();
        view.ui.set_tray_pins(ModelRc::new(VecModel::from(pins)));
        let chevron = Rc::new(Cell::new(0));
        let (left, right) = (Rc::new(RefCell::new(BTreeSet::new())), Rc::new(RefCell::new(BTreeSet::new())));
        let c = chevron.clone();
        view.ui.on_tray_clicked(move |_| c.set(c.get() + 1));
        let l = left.clone();
        view.ui.on_tray_pin_clicked(move |i| {
            l.borrow_mut().insert(i);
        });
        let r = right.clone();
        view.ui.on_tray_pin_menu(move |i, _| {
            r.borrow_mut().insert(i);
        });
        view.render();
        for x in (400..800).step_by(2) {
            click(&view, x, 14);
            view.dispatch(&InputEvent::RightUp { x, y: 14 });
        }
        assert!(chevron.get() > 0, "chevron never reported a click");
        assert_eq!(*left.borrow(), BTreeSet::from([0, 1]));
        assert_eq!(*right.borrow(), BTreeSet::from([0, 1]));
    }

    #[test]
    fn topbar_ink_colors_reach_the_pixels() {
        use slint::Color;

        let view = ContentView::new_with(800, 28, TopBar::new);
        view.ui.set_clock("Пн 2 нояб. 20:31".into());
        // Средняя яркость непрозрачных пикселей правой трети: часы и значки справа.
        let brightness = |px: &[u32]| -> f32 {
            let (mut sum, mut n) = (0.0f32, 0u32);
            for y in 0..28usize {
                for x in (800 * 2 / 3)..800usize {
                    let p = px[y * 800 + x];
                    if p >> 24 > 128 {
                        sum += (((p >> 16) & 0xFF) + ((p >> 8) & 0xFF) + (p & 0xFF)) as f32 / 3.0;
                        n += 1;
                    }
                }
            }
            if n == 0 { -1.0 } else { sum / n as f32 }
        };
        let settle = |view: &ContentView<TopBar>| {
            view.render();
            std::thread::sleep(std::time::Duration::from_millis(350)); // цвет меняется плавно, 250 мс
            view.render()
        };
        view.ui.set_ink_right_to(Color::from_rgb_u8(0, 0, 0));
        let dark = brightness(&settle(&view));
        view.ui.set_ink_right_to(Color::from_rgb_u8(255, 255, 255));
        let light = brightness(&settle(&view));
        assert!(dark >= 0.0 && dark < 70.0, "чёрные значки: яркость {dark}");
        assert!(light > 190.0, "белые значки: яркость {light}");
    }

    #[test]
    fn ink_set_before_the_first_frame_is_used_without_a_transition() {
        use slint::Color;

        let view = ContentView::new_with(800, 28, TopBar::new);
        view.ui.set_app_name("Claude".into());
        view.ui.set_ink_left_to(Color::from_argb_u8(0xE6, 20, 20, 26));
        std::thread::sleep(std::time::Duration::from_millis(350));
        let px = view.render();
        // Название слева: непрозрачные пиксели в первой трети полосы должны быть тёмными.
        let (mut sum, mut n) = (0.0f32, 0u32);
        for y in 0..28usize {
            for x in 40..260usize {
                let p = px[y * 800 + x];
                if p >> 24 > 200 {
                    sum += (((p >> 16) & 0xFF) + ((p >> 8) & 0xFF) + (p & 0xFF)) as f32 / 3.0;
                    n += 1;
                }
            }
        }
        assert!(n > 20, "название должно что-то нарисовать");
        assert!(sum / (n as f32) < 80.0, "название чёрное: средняя яркость {}", sum / n as f32);
    }

    fn spotlight_line(title: &str, index: i32) -> SpotlightLine {
        SpotlightLine {
            header: false,
            gap: false,
            title: title.into(),
            kind: "Приложение".into(),
            glyph: "gear".into(),
            icon: Default::default(),
            has_icon: false,
            index,
        }
    }

    fn spotlight_header(title: &str, gap: bool) -> SpotlightLine {
        SpotlightLine { header: true, gap, index: -1, ..spotlight_line(title, -1) }
    }

    #[test]
    fn spotlight_panel_height_follows_the_rows_and_fits_the_window() {
        use slint::{ModelRc, VecModel};

        let view = ContentView::new_with(680, 560, Spotlight::new);
        view.render();
        assert_eq!(view.ui.get_panel_height(), 80.0, "empty query: only the input capsule");

        let mut lines = vec![spotlight_header("Лучшее совпадение", false), spotlight_line("a", 0)];
        lines.push(spotlight_header("Приложения", true));
        lines.extend((1..5).map(|i| spotlight_line("b", i)));
        lines.push(spotlight_header("Системные настройки", true));
        lines.extend((5..7).map(|i| spotlight_line("c", i)));
        view.ui.set_lines(ModelRc::new(VecModel::from(lines)));
        view.render(); // строки появляются в макете при отрисовке
        let full = view.ui.get_panel_height();
        // 12 + 56 + 9 + заголовки 28 + 36 + 36 + семь строк по 50 + 12
        assert_eq!(full, 12.0 + 56.0 + 9.0 + 28.0 + 36.0 + 36.0 + 7.0 * 50.0 + 12.0);
        assert!(full <= 560.0, "the longest list must fit in the window: {full}");
    }

    #[test]
    fn spotlight_click_reports_the_result_number() {
        use slint::{ModelRc, VecModel};
        use std::cell::Cell;
        use std::rc::Rc;

        let view = ContentView::new_with(680, 560, Spotlight::new);
        let lines = vec![spotlight_header("Лучшее совпадение", false), spotlight_line("one", 0), spotlight_line("two", 1)];
        view.ui.set_lines(ModelRc::new(VecModel::from(lines)));
        let got = Rc::new(Cell::new(-1));
        let slot = got.clone();
        view.ui.on_activated(move |i| slot.set(i));
        let hovered = Rc::new(Cell::new(-1));
        let hv = hovered.clone();
        view.ui.on_hovered(move |i| hv.set(i));
        view.render();
        // Первая строка начинается на 12 + 56 + 9 + 28 = 105, вторая на 155.
        view.dispatch(&InputEvent::Move { x: 300, y: 180 });
        assert_eq!(hovered.get(), 1);
        click(&view, 300, 180);
        assert_eq!(got.get(), 1);
        click(&view, 300, 130);
        assert_eq!(got.get(), 0);
    }

    #[test]
    fn spotlight_preview_replaces_the_list_and_sets_the_panel_height() {
        use slint::{ModelRc, VecModel};

        let view = ContentView::new_with(680, 560, Spotlight::new);
        let lines = vec![spotlight_header("Файлы", false), spotlight_line("one", 0), spotlight_line("two", 1)];
        view.ui.set_lines(ModelRc::new(VecModel::from(lines)));
        view.render();
        let list_height = view.ui.get_panel_height();
        view.ui.set_preview_title("notes.txt".into());
        view.ui.set_preview_meta("TXT · 11 Б".into());
        view.ui.set_preview_text("alpha\nbeta".into());
        view.ui.set_preview_open(true);
        let px = view.render();
        let opened_height = view.ui.get_panel_height();
        // Поле 56 + плита предпросмотра 308 + поля 24: список из двух строк на это время не занимает места.
        assert_eq!(opened_height, 56.0 + 308.0 + 24.0, "preview height replaces the list, was {list_height}");
        let lit = px.iter().filter(|p| *p >> 24 > 0).count();
        assert!(lit > 2000, "preview must paint something, got {lit} pixels");
        view.ui.set_preview_open(false);
        view.render();
        assert_eq!(view.ui.get_panel_height(), list_height, "closing the preview brings the list back");
    }

    #[test]
    fn spotlight_selected_row_is_painted_differently() {
        use slint::{ModelRc, VecModel};

        let view = ContentView::new_with(680, 560, Spotlight::new);
        let lines = vec![spotlight_header("Лучшее совпадение", false), spotlight_line("one", 0), spotlight_line("two", 1)];
        view.ui.set_lines(ModelRc::new(VecModel::from(lines)));
        view.ui.set_selected(0);
        view.render();
        std::thread::sleep(std::time::Duration::from_millis(150)); // подсветка проявляется 90 мс
        let px = view.render();
        let alpha_at = |y: usize| px[y * 680 + 340] >> 24;
        assert!(alpha_at(110) > 150, "selected row must have a highlight, got {}", alpha_at(110));
        assert!(alpha_at(160) < 30, "unselected row stays clear, got {}", alpha_at(160));
    }

    #[test]
    fn volume_slider_follows_press_and_drag_and_clamps() {
        use std::cell::Cell;
        use std::rc::Rc;

        let view = ContentView::new_with(240, 52, VolumePopup::new);
        let last = Rc::new(Cell::new(-1.0f32));
        let slot = last.clone();
        view.ui.on_level_changed(move |v| slot.set(v));
        view.render();
        // Дорожка от 52 до 188, ручка идёт от 61 до 179 (центр ручки).
        for ev in [InputEvent::Move { x: 120, y: 26 }, InputEvent::Down { x: 120, y: 26 }] {
            view.dispatch(&ev);
        }
        assert!((last.get() - 0.5).abs() < 0.02, "press in the middle: {}", last.get());
        view.dispatch(&InputEvent::Move { x: 400, y: 26 });
        assert_eq!(last.get(), 1.0, "dragging past the right end clamps to full");
        view.dispatch(&InputEvent::Move { x: -50, y: 90 });
        assert_eq!(last.get(), 0.0, "dragging past the left end clamps to zero");
        view.dispatch(&InputEvent::Up { x: -50, y: 90 });
        last.set(-1.0);
        view.dispatch(&InputEvent::Move { x: 150, y: 26 });
        assert_eq!(last.get(), -1.0, "moving without the button pressed changes nothing");
    }

    #[test]
    fn volume_mute_button_and_level_display() {
        use std::cell::Cell;
        use std::rc::Rc;

        let view = ContentView::new_with(240, 52, VolumePopup::new);
        let toggled = Rc::new(Cell::new(false));
        let flag = toggled.clone();
        view.ui.on_mute_toggled(move || flag.set(true));
        view.ui.set_level(0.5);
        view.render();
        click(&view, 26, 26);
        assert!(toggled.get());
        let bright = |view: &ContentView<VolumePopup>| view.render().iter().filter(|p| *p >> 24 > 200).count();
        let half = bright(&view);
        view.ui.set_level(1.0);
        assert!(bright(&view) > half, "a fuller slider paints more");
        view.ui.set_muted(true);
        let muted = bright(&view);
        assert!(muted < half, "muted slider paints an empty track");
    }

    #[test]
    fn calendar_buttons_and_today_circle() {
        use slint::{ModelRc, VecModel};
        use std::cell::Cell;
        use std::rc::Rc;

        let view = ContentView::new_with(280, 292, CalendarPopup::new);
        let days: Vec<CalDay> = crate::calendar::month_grid(2026, 11, (2026, 11, 2))
            .into_iter()
            .map(|d| CalDay { text: d.day.to_string().into(), in_month: d.in_month, today: d.today })
            .collect();
        view.ui.set_days(ModelRc::new(VecModel::from(days)));
        view.ui.set_month_title("ноябрь 2026".into());
        let (prev, next, today) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let (p, n, t) = (prev.clone(), next.clone(), today.clone());
        view.ui.on_prev(move || p.set(p.get() + 1));
        view.ui.on_next(move || n.set(n.get() + 1));
        view.ui.on_today(move || t.set(t.get() + 1));
        let px = view.render();
        click(&view, 165, 30);
        click(&view, 252, 30);
        click(&view, 205, 30);
        assert_eq!(today.get(), 0, "today is hidden in the current month");
        view.ui.set_other_month(true);
        view.render();
        click(&view, 205, 30);
        assert_eq!((prev.get(), next.get(), today.get()), (1, 1, 1));
        // Сегодня (2 ноября) во втором ряду первой колонки: синий кружок слева от цифры.
        let p = px[129 * 280 + 22];
        let (a, r, b) = (p >> 24, (p >> 16) & 0xff, p & 0xff);
        assert!(a > 200 && b > r + 40, "today must be an accent circle, got a={a} r={r} b={b}");
        // Соседний день без кружка остаётся прозрачным в том же месте.
        assert!(px[129 * 280 + 58] >> 24 < 30);
    }

    #[test]
    fn right_side_buttons_report_their_centres() {
        use std::cell::Cell;
        use std::rc::Rc;

        let view = ContentView::new_with(800, 28, TopBar::new);
        view.ui.set_clock("Пн 2 нояб. 20:31".into());
        view.ui.set_input_lang("RU".into());
        view.ui.set_bluetooth(2);
        view.ui.set_tray_enabled(false);
        view.ui.set_status_order(slint::ModelRc::new(slint::VecModel::from(vec![4, 5, 6, 7, 8, 9])));
        let (volume, clock, bluetooth, lang) = (Rc::new(Cell::new(-1.0f32)), Rc::new(Cell::new(-1.0f32)), Rc::new(Cell::new(-1.0f32)), Rc::new(Cell::new(0)));
        let (v, c, b, l) = (volume.clone(), clock.clone(), bluetooth.clone(), lang.clone());
        view.ui.on_volume_clicked(move |x| v.set(x));
        view.ui.on_clock_clicked(move |x| c.set(x));
        view.ui.on_bluetooth_clicked(move |x| b.set(x));
        view.ui.on_lang_clicked(move || l.set(l.get() + 1));
        view.render();
        // Справа налево: часы, центр управления (24), Spotlight (24), сеть (24), звук (24), раскладка (28), Bluetooth (22).
        let right = 800.0 - 8.0;
        // Часы: клик у самого правого края (внутри отступа 6 px от текста).
        click(&view, (right - 3.0) as i32, 14);
        assert!(clock.get() > 600.0 && clock.get() < right, "clock centre {}", clock.get());
        // Звук левее часов: 8 + центр управления 24 + 8 + Spotlight 24 + 8 + сеть 24 + 8 + половина кнопки звука.
        let clock_left = clock.get() - (right - clock.get());
        let volume_centre = clock_left - 8.0 - 24.0 - 8.0 - 24.0 - 8.0 - 24.0 - 8.0 - 12.0;
        click(&view, volume_centre as i32, 14);
        assert!((volume.get() - volume_centre).abs() < 1.5, "volume centre {} expected {volume_centre}", volume.get());
        click(&view, (volume_centre - 12.0 - 8.0 - 14.0) as i32, 14);
        assert_eq!(lang.get(), 1);
        click(&view, (volume_centre - 12.0 - 8.0 - 28.0 - 8.0 - 11.0) as i32, 14);
        assert!(bluetooth.get() > 0.0);
    }

    #[test]
    fn control_center_tiles_sliders_and_media_buttons_report_clicks() {
        use std::cell::Cell;
        use std::rc::Rc;

        let view = ContentView::new_with(316, 408, ControlCenter::new);
        let counter = || Rc::new(Cell::new(0));
        let (bt, dark, focus, night, cast) = (counter(), counter(), counter(), counter(), counter());
        let (play, level) = (counter(), Rc::new(Cell::new(-1.0f32)));
        let (b, d, f, n, c, pl, l) = (bt.clone(), dark.clone(), focus.clone(), night.clone(), cast.clone(), play.clone(), level.clone());
        view.ui.on_bluetooth_clicked(move || b.set(b.get() + 1));
        let expanded = counter();
        let e = expanded.clone();
        view.ui.on_bluetooth_expand(move || e.set(e.get() + 1));
        view.ui.on_dark_clicked(move || d.set(d.get() + 1));
        view.ui.on_focus_clicked(move || f.set(f.get() + 1));
        view.ui.on_night_clicked(move || n.set(n.get() + 1));
        view.ui.on_cast_clicked(move || c.set(c.get() + 1));
        view.ui.on_media_play_pause(move || pl.set(pl.get() + 1));
        view.ui.on_level_changed(move |v| l.set(v));
        view.ui.set_media_active(true);
        view.render();
        assert_eq!(view.ui.get_panel_height(), crate::popups::cc_height(false), "Rust and Slint agree on the height");
        click(&view, 189, 42); // значок Bluetooth в круге переключает радио
        click(&view, 240, 42); // остальная плитка открывает список устройств
        assert_eq!(expanded.get(), 1);
        click(&view, 83, 246); // тёмная тема после медиа
        click(&view, 233, 246); // не беспокоить
        click(&view, 83, 314); // ночной свет
        click(&view, 233, 314); // трансляция
        assert_eq!((bt.get(), dark.get(), focus.get(), night.get(), cast.get()), (1, 1, 1, 1, 1));
        // Громкость идёт сразу после связи, если яркость недоступна.
        click(&view, 12 + 52 + (292 - 104) / 2, 106);
        assert!((level.get() - 0.5).abs() < 0.08, "slider middle gave {}", level.get());
        // Медиа под ползунком.
        click(&view, 12 + 292 - 80 + 18, 140 + 34);
        assert_eq!(play.get(), 1);
    }

    #[test]
    fn control_center_brightness_row_pushes_the_rest_down() {
        use std::cell::Cell;
        use std::rc::Rc;

        let view = ContentView::new_with(316, 468, ControlCenter::new);
        view.ui.set_has_brightness(true);
        let (bright, level) = (Rc::new(Cell::new(-1.0f32)), Rc::new(Cell::new(-1.0f32)));
        let (b, l) = (bright.clone(), level.clone());
        view.ui.on_brightness_changed(move |v| b.set(v));
        view.ui.on_level_changed(move |v| l.set(v));
        view.render();
        assert_eq!(view.ui.get_panel_height(), crate::popups::cc_height(true));
        click(&view, 12 + 52 + (292 - 104) / 2, 106); // ползунок яркости
        click(&view, 12 + 52 + (292 - 104) / 4, 166); // громкость ниже на 60
        assert!((bright.get() - 0.5).abs() < 0.08, "brightness gave {}", bright.get());
        assert!((level.get() - 0.25).abs() < 0.08, "volume gave {}", level.get());
    }

    #[test]
    fn logo_click_invokes_callback() {
        use crate::host::InputEvent;
        use std::cell::Cell;
        use std::rc::Rc;

        let view = ContentView::new_with(800, 28, TopBar::new);
        let clicked = Rc::new(Cell::new(false));
        let flag = clicked.clone();
        view.ui.on_logo_clicked(move |_| flag.set(true));
        view.render();
        for ev in [InputEvent::Move { x: 20, y: 14 }, InputEvent::Down { x: 20, y: 14 }, InputEvent::Up { x: 20, y: 14 }] {
            view.dispatch(&ev);
        }
        assert!(clicked.get());
    }
}
