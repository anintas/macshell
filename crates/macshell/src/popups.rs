//! Всплывающие окна верхней полосы: громкость (под динамиком) и календарь (под часами). Окно висит под полосой
//! на стекле, мышь захвачена: клик вне окна и Esc закрывают его.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use anyhow::{Result, anyhow};
use capture::RectI;
use glass::{Bitmap, GlassParams, cpu};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};

use crate::bar::{menu_glass, popup_glass};
use crate::calendar;
use crate::clock::LocalTime;
use crate::host::{InputEvent, LayeredWindow};
use crate::audio::{self, AudioDevice, Flow};
use crate::bluetooth;
use crate::lists::{self, BtList, Click, Page, WifiList};
use crate::slint_view::{CalDay, CalendarPopup, ContentView, ControlCenter, SoundRow, VolumePopup};
use crate::status::VolumeControl;
use crate::{brightness, media};

/// Где стоит полоса: от неё считается место окна.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub bar: RectI,
    pub bar_height: i32,
    pub scale: f32,
}

/// Левый край окна шириной `width` с центром под `center_x`, не выходя за экран шириной `screen_w` (физические пиксели).
pub fn place_x(center_x: i32, width: i32, screen_w: i32) -> i32 {
    (center_x - width / 2).clamp(0, (screen_w - width).max(0))
}

/// Окно на стекле под полосой.
struct Frame {
    win: LayeredWindow,
    glass: Bitmap,
    size: (i32, i32),
    previous: Option<isize>,
}

impl Frame {
    /// Окно `w_pt` x `h_pt` логических пикселей с центром под `center_x` (логические пиксели).
    fn open(anchor: Anchor, center_x: f32, w_pt: f32, h_pt: f32, glass: &GlassParams) -> Result<Frame> {
        Self::open_with_focus(anchor, center_x, w_pt, h_pt, glass, false)
    }

    fn open_with_focus(anchor: Anchor, center_x: f32, w_pt: f32, h_pt: f32, glass: &GlassParams, focusable: bool) -> Result<Frame> {
        let s = anchor.scale;
        let (w, h) = ((w_pt * s).round() as i32, (h_pt * s).ceil() as i32);
        let x = place_x((center_x * s) as i32, w, anchor.bar.w);
        let y = anchor.bar.y + anchor.bar_height + (4.0 * s).round() as i32;
        let previous = focusable.then(|| unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize });
        let rect = RectI { x, y, w, h };
        let mut win = if focusable { LayeredWindow::new_focusable(rect)? } else { LayeredWindow::new(rect)? };
        win.animate_overlay(true, (-6.0 * s).round() as i32);
        let glass = popup_glass(win.rect(), &glass.scaled(s))?;
        Ok(Frame { win, glass, size: (w, h), previous })
    }

    fn present<C: ComponentHandle>(&self, view: &ContentView<C>) -> Result<()> {
        if let Some(content) = view.render_if_changed() {
            let mut out = self.glass.clone();
            cpu::composite_over(&mut out, &content);
            self.win.present(&out)?;
        }
        Ok(())
    }

    /// Раздаёт мышь окну; `true`, если пора закрыть (клик вне окна или Esc).
    fn pump<C: ComponentHandle>(&self, view: &ContentView<C>) -> bool {
        let mut close = false;
        for ev in self.win.take_input() {
            if let InputEvent::Down { x, y } = ev {
                close |= x < 0 || y < 0 || x >= self.size.0 || y >= self.size.1;
            }
            view.dispatch(&ev);
        }
        close || unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } as u16 & 0x8000 != 0
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        self.win.capture_mouse(false);
        if self.win.is_foreground() {
            if let Some(previous) = self.previous.filter(|p| *p != 0) { crate::host::bring_to_front(previous); }
        }
    }
}

pub(crate) fn popup_glass_params() -> GlassParams {
    GlassParams { radius: 20.0, bevel: 14.0, refraction: 10.0, ..menu_glass() }
}

/// Что делает строка под ползунком в окне звука.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundAction {
    /// Заголовок раздела: не нажимается.
    Header,
    /// Сделать устройство (по идентификатору) устройством по умолчанию.
    Device(String),
    /// Открыть параметры звука Windows.
    Settings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundLine {
    pub text: String,
    pub action: SoundAction,
    pub current: bool,
}

/// Строки под ползунком: «Вывод» с устройствами, «Ввод» с устройствами (если они есть) и ссылка на параметры.
pub fn sound_lines(outputs: &[AudioDevice], inputs: &[AudioDevice]) -> Vec<SoundLine> {
    let mut lines = Vec::new();
    for (title, devices) in [("Вывод", outputs), ("Ввод", inputs)] {
        if devices.is_empty() {
            continue;
        }
        lines.push(SoundLine { text: title.into(), action: SoundAction::Header, current: false });
        lines.extend(devices.iter().map(|d| SoundLine { text: d.name.clone(), action: SoundAction::Device(d.id.clone()), current: d.default }));
    }
    lines.push(SoundLine { text: "Параметры звука…".into(), action: SoundAction::Settings, current: false });
    lines
}

/// Высота окна звука в логических пикселях: ползунок 52, отступ 4, строки (заголовок 24, остальные 28), низ 8.
pub fn sound_height(lines: &[SoundLine]) -> f32 {
    56.0 + lines.iter().map(|l| if l.action == SoundAction::Header { 24.0 } else { 28.0 }).sum::<f32>() + 8.0
}

/// Окно звука: ползунок и кнопка «без звука», устройства вывода и ввода, параметры.
pub struct VolumeWin {
    frame: Frame,
    view: ContentView<VolumePopup>,
    control: RefCell<VolumeControl>,
    lines: RefCell<Vec<SoundLine>>,
    requested: Rc<Cell<Option<f32>>>,
    mute_toggled: Rc<Cell<bool>>,
    row_clicked: Rc<Cell<i32>>,
}

impl VolumeWin {
    pub const WIDTH: f32 = 240.0;

    pub fn open(anchor: Anchor, center_x: f32) -> Result<VolumeWin> {
        let control = VolumeControl::open().ok_or_else(|| anyhow!("нет устройства вывода звука"))?;
        let lines = sound_lines(&audio::list(Flow::Output), &audio::list(Flow::Input));
        let frame = Frame::open(anchor, center_x, Self::WIDTH, sound_height(&lines), &popup_glass_params())?;
        let view = ContentView::new_with(frame.size.0 as u32, frame.size.1 as u32, VolumePopup::new);
        view.set_scale_factor(anchor.scale);
        let requested = Rc::new(Cell::new(None));
        let slot = requested.clone();
        view.ui.on_level_changed(move |v| slot.set(Some(v)));
        let mute_toggled = Rc::new(Cell::new(false));
        let slot = mute_toggled.clone();
        view.ui.on_mute_toggled(move || slot.set(true));
        let row_clicked = Rc::new(Cell::new(-1));
        let slot = row_clicked.clone();
        view.ui.on_row_clicked(move |i| slot.set(i));
        let win = VolumeWin { frame, view, control: RefCell::new(control), lines: RefCell::new(lines), requested, mute_toggled, row_clicked };
        win.show_lines();
        win.sync();
        win.frame.present(&win.view)?;
        win.frame.win.capture_mouse(true);
        Ok(win)
    }

    fn show_lines(&self) {
        let rows: Vec<SoundRow> = self
            .lines
            .borrow()
            .iter()
            .map(|l| SoundRow { text: l.text.as_str().into(), header: l.action == SoundAction::Header, current: l.current })
            .collect();
        self.view.ui.set_rows(ModelRc::new(VecModel::from(rows)));
    }

    /// Показывает то, что сейчас в системе (громкость могли поменять кнопками клавиатуры).
    pub fn sync(&self) {
        let c = self.control.borrow();
        self.view.ui.set_level(c.level());
        self.view.ui.set_muted(c.muted());
    }

    /// Один шаг: ввод, применение громкости, выбор устройства, кадр. `true`, если окно пора закрыть.
    pub fn tick(&self) -> Result<bool> {
        let mut close = self.frame.pump(&self.view);
        if let Some(level) = self.requested.take() {
            self.control.borrow().set_level(level);
            self.sync();
        }
        if self.mute_toggled.take() {
            let c = self.control.borrow();
            c.set_muted(!c.muted());
            drop(c);
            self.sync();
        }
        let row = self.row_clicked.replace(-1);
        if row >= 0 {
            let action = self.lines.borrow().get(row as usize).map(|l| l.action.clone());
            match action {
                Some(SoundAction::Device(id)) => {
                    if audio::set_default(&id) {
                        crate::status::notify(); // значок громкости перечитает новое устройство
                        if let Some(c) = VolumeControl::open() {
                            *self.control.borrow_mut() = c;
                        }
                        *self.lines.borrow_mut() = sound_lines(&audio::list(Flow::Output), &audio::list(Flow::Input));
                        self.show_lines();
                        self.sync();
                    }
                }
                Some(SoundAction::Settings) => {
                    crate::bar::launch("ms-settings:sound");
                    close = true;
                }
                _ => {}
            }
        }
        self.frame.present(&self.view)?;
        Ok(close)
    }

    pub fn animating(&self) -> bool {
        self.view.animating()
    }
}

/// Что просит Центр управления снаружи: открыть страницу параметров Windows (окно после этого закрывается).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CcAction {
    Wifi,
    Bluetooth,
    Dark,
    Night,
    Display,
    Settings,
    Focus,
    Cast,
    /// Страницы со списком: открыть Bluetooth или Wi-Fi, вернуться к плиткам, параметры Windows этой страницы.
    BtExpand,
    WifiExpand,
    ListToggle,
    ListBack,
    ListSettings,
}

/// Первым в паре идёт открытие подробностей, вторым — переключатель радио.
fn tile_keyboard_action(index: i32) -> Option<CcAction> {
    match index {
        0 => Some(CcAction::WifiExpand), 1 => Some(CcAction::Wifi),
        2 => Some(CcAction::BtExpand), 3 => Some(CcAction::Bluetooth),
        7 => Some(CcAction::Dark), 8 => Some(CcAction::Focus),
        9 => Some(CcAction::Night), 10 => Some(CcAction::Cast),
        11 => Some(CcAction::Display), 12 => Some(CcAction::Settings), _ => None,
    }
}

/// Куда ведёт плитка, которая сама ничего не переключает; `None` у переключателей.
fn cc_target(action: CcAction) -> Option<&'static str> {
    match action {
        CcAction::Night => Some("ms-settings:nightlight"),
        CcAction::Display => Some("ms-settings:display"),
        CcAction::Settings => Some("ms-settings:"),
        CcAction::Cast => Some("ms-settings-connectabledevices:devicediscovery"),
        CcAction::Wifi | CcAction::Bluetooth | CcAction::Dark | CcAction::Focus | CcAction::BtExpand | CcAction::WifiExpand | CcAction::ListToggle | CcAction::ListBack | CcAction::ListSettings => None,
    }
}

/// Подпись плитки Bluetooth по состоянию радио.
pub fn bluetooth_caption(state: i32) -> &'static str {
    match state {
        crate::bluetooth::ON => "Вкл.",
        crate::bluetooth::OFF => "Выкл.",
        _ => "Нет радио",
    }
}

/// Подпись плитки сети: 0 нет сети, 1 Wi-Fi, 2 проводная.
pub fn network_caption(wifi: i32) -> &'static str {
    match wifi {
        0 => "Нет сети",
        1 => "Подключено",
        _ => "Проводная сеть",
    }
}

/// Высота Центра управления в логических пикселях: три ряда плиток, ползунок яркости (если есть), громкость,
/// «Сейчас играет» и нижние плитки. Должна совпадать с `panel-height` в `controlcenter.slint`.
pub fn cc_height(has_brightness: bool) -> f32 {
    let sound_y = if has_brightness { 276.0 } else { 216.0 };
    sound_y + 60.0 + 76.0 + 44.0 + 12.0
}

/// Страница, которую показывает окно: плитки или список.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CcPage {
    Tiles = 0,
    Bluetooth = 1,
    Wifi = 2,
}

/// Центр управления: плитки Wi-Fi, Bluetooth, тёмной темы, «Не беспокоить», ночного света и трансляции, ползунки
/// яркости и громкости, «Сейчас играет» и ссылки на параметры. Wi-Fi и Bluetooth открываются списком (сети,
/// устройства); тот же список может стоять отдельным окном под значком в строке меню.
pub struct ControlCenterWin {
    frame: Frame,
    view: ContentView<ControlCenter>,
    control: Option<VolumeControl>,
    wifi: i32,
    /// Wi-Fi включили или выключили в этом окне: плитка показывает это, пока панель не перечитала сеть.
    wifi_override: Cell<Option<bool>>,
    level: Rc<Cell<Option<f32>>>,
    brightness: Rc<Cell<Option<f32>>>,
    mute_toggled: Rc<Cell<bool>>,
    actions: Rc<RefCell<Vec<CcAction>>>,
    media_command: Rc<Cell<Option<media::Command>>>,
    /// Ответ на запрос к плееру, который ещё в пути.
    media_pending: RefCell<Option<std::sync::mpsc::Receiver<Option<media::Now>>>>,
    /// Когда в следующий раз спросить плеер, что играет (пока окно открыто, раз в две секунды).
    media_poll_at: Cell<std::time::Instant>,
    page: Cell<CcPage>,
    bt: RefCell<BtList>,
    wifi_list: RefCell<WifiList>,
    list_row: Rc<Cell<i32>>,
    /// Состояние радио, с которым в последний раз строился список: смена обновляет его сразу.
    last_radio: Cell<(i32, Option<bool>)>,
}

impl ControlCenterWin {
    pub const WIDTH: f32 = 316.0;

    /// `wifi`: состояние сети с панели (0 нет, 1 Wi-Fi, 2 проводная).
    pub fn open(anchor: Anchor, center_x: f32, wifi: i32) -> Result<ControlCenterWin> {
        Self::open_page(anchor, center_x, wifi, CcPage::Tiles)
    }

    /// Окно только со списком под значком Bluetooth или Wi-Fi в строке меню: без плиток и без стрелки «назад».
    pub fn open_list(anchor: Anchor, center_x: f32, wifi: i32, page: CcPage) -> Result<ControlCenterWin> {
        Self::open_page(anchor, center_x, wifi, page)
    }

    fn open_page(anchor: Anchor, center_x: f32, wifi: i32, start: CcPage) -> Result<ControlCenterWin> {
        let standalone = start != CcPage::Tiles;
        let has_brightness = brightness::supported();
        let mut bt = BtList::new(6);
        let mut wifi_list = WifiList::new(6, wifi == 2);
        // Окно постоянного размера: отдельный список по числу строк при открытии, Центр управления как есть.
        let (height, body) = if standalone {
            let rows = if start == CcPage::Bluetooth { bt.devices.len() } else { wifi_list.networks.len() }.min(6);
            (lists::height(rows), lists::body(rows))
        } else {
            let h = cc_height(has_brightness);
            (h, h - 144.0)
        };
        bt.max_rows = lists::max_rows(body);
        wifi_list.max_rows = lists::max_rows(body);
        let frame = Frame::open_with_focus(anchor, center_x, Self::WIDTH, height, &popup_glass_params(), true)?;
        let view = ContentView::new_with(frame.size.0 as u32, frame.size.1 as u32, ControlCenter::new);
        view.set_scale_factor(anchor.scale);
        view.ui.set_has_brightness(has_brightness);
        view.ui.set_standalone(standalone);
        view.ui.set_page(start as i32);
        view.ui.set_list_body_h(body);
        let level = Rc::new(Cell::new(None));
        let slot = level.clone();
        view.ui.on_level_changed(move |v| slot.set(Some(v)));
        let brightness_req = Rc::new(Cell::new(None));
        let slot = brightness_req.clone();
        view.ui.on_brightness_changed(move |v| slot.set(Some(v)));
        let mute_toggled = Rc::new(Cell::new(false));
        let slot = mute_toggled.clone();
        view.ui.on_mute_toggled(move || slot.set(true));
        let actions = Rc::new(RefCell::new(Vec::new()));
        let push = |action: CcAction| {
            let list = actions.clone();
            move || list.borrow_mut().push(action)
        };
        view.ui.on_wifi_clicked(push(CcAction::Wifi));
        view.ui.on_wifi_expand(push(CcAction::WifiExpand));
        view.ui.on_bluetooth_clicked(push(CcAction::Bluetooth));
        view.ui.on_bluetooth_expand(push(CcAction::BtExpand));
        view.ui.on_list_toggled(push(CcAction::ListToggle));
        view.ui.on_list_back(push(CcAction::ListBack));
        view.ui.on_list_settings(push(CcAction::ListSettings));
        let list_row = Rc::new(Cell::new(-1));
        let slot = list_row.clone();
        view.ui.on_list_row_clicked(move |i| slot.set(i));
        view.ui.on_dark_clicked(push(CcAction::Dark));
        view.ui.on_focus_clicked(push(CcAction::Focus));
        view.ui.on_night_clicked(push(CcAction::Night));
        view.ui.on_cast_clicked(push(CcAction::Cast));
        view.ui.on_display_clicked(push(CcAction::Display));
        view.ui.on_settings_clicked(push(CcAction::Settings));
        let media_command = Rc::new(Cell::new(None));
        for (cmd, register) in [
            (media::Command::Previous, 0),
            (media::Command::PlayPause, 1),
            (media::Command::Next, 2),
        ] {
            let slot = media_command.clone();
            match register {
                0 => view.ui.on_media_previous(move || slot.set(Some(cmd))),
                1 => view.ui.on_media_play_pause(move || slot.set(Some(cmd))),
                _ => view.ui.on_media_next(move || slot.set(Some(cmd))),
            }
        }
        let win = ControlCenterWin {
            frame,
            view,
            control: VolumeControl::open(),
            wifi,
            wifi_override: Cell::new(None),
            level,
            brightness: brightness_req,
            mute_toggled,
            actions,
            media_command,
            media_pending: RefCell::new(Some(media::request())),
            media_poll_at: Cell::new(std::time::Instant::now() + MEDIA_POLL),
            page: Cell::new(start),
            bt: RefCell::new(bt),
            wifi_list: RefCell::new(wifi_list),
            list_row,
            last_radio: Cell::new((-1, None)),
        };
        win.show_list();
        win.sync();
        win.frame.present(&win.view)?;
        win.frame.win.capture_mouse(true);
        win.frame.win.focus();
        Ok(win)
    }

    /// Показывает то, что сейчас в системе: громкость, яркость, Bluetooth, тема, «Не беспокоить», сеть.
    pub fn sync(&self) {
        let ui = &self.view.ui;
        if let Some(c) = &self.control {
            ui.set_level(c.level());
            ui.set_muted(c.muted());
        }
        if brightness::supported() {
            ui.set_brightness(brightness::level());
        }
        let bt = bluetooth::state();
        ui.set_bluetooth_on(bt == bluetooth::ON);
        ui.set_bluetooth_subtitle(bluetooth_caption(bt).into());
        ui.set_dark_on(crate::theme::is_dark());
        ui.set_focus_on(crate::focus::is_on());
        match self.wifi_override.get() {
            Some(on) => {
                ui.set_wifi_on(on);
                ui.set_wifi_subtitle(if on { "Вкл." } else { "Выкл." }.into());
            }
            None => {
                ui.set_wifi_on(self.wifi != 0);
                ui.set_wifi_subtitle(network_caption(self.wifi).into());
            }
        }
    }

    /// Данные открытой страницы в интерфейс.
    fn show_list(&self) {
        let page: Page = match self.page.get() {
            CcPage::Tiles => return,
            CcPage::Bluetooth => self.bt.borrow().page(),
            CcPage::Wifi => self.wifi_list.borrow().page(),
        };
        let ui = &self.view.ui;
        ui.set_list_title(page.title.into());
        ui.set_list_heading(page.heading.into());
        ui.set_list_on(page.on);
        ui.set_list_available(page.available);
        ui.set_list_state(page.state.into());
        ui.set_list_empty(page.empty.as_str().into());
        ui.set_list_footer(page.footer.as_str().into());
        ui.set_list_rows(ModelRc::new(VecModel::from(page.rows)));
    }

    fn open_list_page(&self, page: CcPage) {
        self.page.set(page);
        self.view.ui.set_page(page as i32);
        self.view.ui.set_keyboard_focus(0);
        match page {
            CcPage::Bluetooth => {
                self.bt.borrow_mut().poll(true);
            }
            CcPage::Wifi => {
                self.wifi_list.borrow_mut().poll(true);
            }
            CcPage::Tiles => {}
        }
        self.show_list();
    }

    /// Выключатель в шапке страницы.
    fn toggle_list(&self) {
        match self.page.get() {
            CcPage::Bluetooth => self.toggle_bluetooth(),
            CcPage::Wifi => self.toggle_wifi(),
            CcPage::Tiles => {}
        }
    }

    fn toggle_bluetooth(&self) {
        let state = bluetooth::state();
        if state != bluetooth::NONE {
            bluetooth::set_enabled(state != bluetooth::ON);
        }
    }

    fn toggle_wifi(&self) {
        let on = self.wifi_override.get().unwrap_or_else(|| self.wifi_list.borrow().radio().unwrap_or(self.wifi != 0));
        crate::wifi::set_enabled(!on);
        self.wifi_override.set(Some(!on));
        self.wifi_list.borrow_mut().poll(true);
    }

    /// Ответ плеера в карточку «Сейчас играет».
    fn show_media(&self, now: Option<media::Now>) {
        let ui = &self.view.ui;
        ui.set_media_active(now.is_some());
        ui.set_media_playing(now.as_ref().is_some_and(|n| n.playing));
        ui.set_media_title(now.as_ref().map_or("Ничего не играет".to_string(), |n| n.title.clone()).into());
        ui.set_media_artist(if now.is_some() { media::subtitle(&now) } else { String::new() }.into());
    }

    fn key(&self, vk: u32, shift: bool) -> bool {
        if vk == 0x1b { return true; }
        let ui = &self.view.ui;
        let current = ui.get_keyboard_focus();
        let count = if self.page.get() == CcPage::Tiles { 13 } else {
            3 + if ui.get_list_on() { ui.get_list_rows().row_count() as i32 } else { 0 }
        };
        if matches!(vk, 0x09 | 0x26 | 0x28) {
            let back = vk == 0x26 || (vk == 0x09 && shift);
            let mut next = if current < 0 { if back { 0 } else { -1 } } else { current };
            loop {
                next = (next + if back { -1 } else { 1 }).rem_euclid(count);
                if !(self.page.get() == CcPage::Tiles && next == 4 && !ui.get_has_brightness())
                    && !(self.page.get() != CcPage::Tiles && next == 0 && ui.get_standalone()) { break; }
            }
            ui.set_keyboard_focus(next);
        } else if matches!(vk, 0x25 | 0x27) && self.page.get() == CcPage::Tiles {
            let delta = if vk == 0x25 { -0.05 } else { 0.05 };
            if current == 4 && ui.get_has_brightness() { self.brightness.set(Some((ui.get_brightness() + delta).clamp(0.0, 1.0))); }
            if current == 5 { self.level.set(Some((ui.get_level() + delta).clamp(0.0, 1.0))); }
        } else if matches!(vk, 0x0d | 0x20) && current >= 0 {
            if self.page.get() == CcPage::Tiles {
                let action = tile_keyboard_action(current);
                if let Some(action) = action { self.actions.borrow_mut().push(action); }
                if current == 6 && ui.get_media_active() { self.media_command.set(Some(media::Command::PlayPause)); }
            } else {
                let rows = if ui.get_list_on() { ui.get_list_rows().row_count() as i32 } else { 0 };
                if current == 0 && !ui.get_standalone() { self.actions.borrow_mut().push(CcAction::ListBack); }
                else if current == 1 && ui.get_list_available() { self.actions.borrow_mut().push(CcAction::ListToggle); }
                else if (2..2 + rows).contains(&current) { self.list_row.set(current - 2); }
                else if current == 2 + rows { self.actions.borrow_mut().push(CcAction::ListSettings); }
            }
        }
        false
    }

    /// Один шаг: ввод, переключатели, кадр. Возвращает `(закрыть окно, страница параметров для открытия)`.
    pub fn tick(&self) -> Result<(bool, Option<&'static str>)> {
        let mut close = false;
        for ev in self.frame.win.take_input() {
            match ev {
                InputEvent::Key { vk, shift, .. } => close |= self.key(vk, shift),
                InputEvent::Char(' ') => close |= self.key(0x20, false),
                InputEvent::Down { x, y } => {
                    close |= x < 0 || y < 0 || x >= self.frame.size.0 || y >= self.frame.size.1;
                    self.view.dispatch(&ev);
                }
                _ => self.view.dispatch(&ev),
            }
        }
        let mut open = None;
        if let Some(level) = self.level.take() {
            if let Some(c) = &self.control {
                c.set_level(level);
            }
        }
        if let Some(b) = self.brightness.take() {
            brightness::set_level(b);
        }
        if self.mute_toggled.take() {
            if let Some(c) = &self.control {
                c.set_muted(!c.muted());
            }
        }
        if let Some(cmd) = self.media_command.take() {
            *self.media_pending.borrow_mut() = Some(media::send(cmd));
        }
        let answer = self.media_pending.borrow().as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(now) = answer {
            *self.media_pending.borrow_mut() = None;
            self.show_media(now);
            self.media_poll_at.set(std::time::Instant::now() + MEDIA_POLL);
        } else if self.media_pending.borrow().is_none() && std::time::Instant::now() >= self.media_poll_at.get() {
            *self.media_pending.borrow_mut() = Some(media::request());
        }
        let actions: Vec<CcAction> = self.actions.borrow_mut().drain(..).collect();
        for action in actions {
            match action {
                CcAction::Bluetooth => self.toggle_bluetooth(),
                CcAction::Wifi => self.toggle_wifi(),
                CcAction::ListToggle => self.toggle_list(),
                CcAction::BtExpand => self.open_list_page(CcPage::Bluetooth),
                CcAction::WifiExpand => self.open_list_page(CcPage::Wifi),
                CcAction::ListBack => {
                    self.page.set(CcPage::Tiles);
                    self.view.ui.set_page(0);
                    self.view.ui.set_keyboard_focus(0);
                }
                CcAction::ListSettings => {
                    open = Some(if self.page.get() == CcPage::Wifi { "ms-settings:network-wifi" } else { "ms-settings:bluetooth" });
                    close = true;
                }
                CcAction::Dark => crate::theme::set_dark(!crate::theme::is_dark()),
                CcAction::Focus => crate::focus::set(!crate::focus::is_on()),
                other => {
                    open = cc_target(other);
                    close = true;
                }
            }
        }
        let row = self.list_row.replace(-1);
        if row >= 0 {
            let click = match self.page.get() {
                CcPage::Bluetooth => self.bt.borrow_mut().click(row as usize),
                CcPage::Wifi => self.wifi_list.borrow_mut().click(row as usize),
                CcPage::Tiles => Click::Nothing,
            };
            match click {
                Click::Settings => {
                    open = Some("ms-settings:bluetooth");
                    close = true;
                }
                Click::Networks => {
                    open = Some("ms-availablenetworks:");
                    close = true;
                }
                Click::Nothing => {}
            }
            self.show_list();
        }
        if self.page.get() != CcPage::Tiles {
            // Список обновляется, пока страница открыта; смена радио (включили или выключили) обновляет его сразу.
            let radio = (bluetooth::state(), self.wifi_list.borrow().radio());
            let radio_changed = self.last_radio.replace(radio) != radio;
            let changed = match self.page.get() {
                CcPage::Bluetooth => self.bt.borrow_mut().poll(radio_changed),
                _ => self.wifi_list.borrow_mut().poll(radio_changed),
            };
            if changed || radio_changed {
                self.show_list();
            }
        }
        self.sync();
        self.frame.present(&self.view)?;
        Ok((close, open))
    }

    pub fn animating(&self) -> bool {
        self.view.animating()
    }
}

/// Как часто открытый Центр управления спрашивает плеер, что играет.
const MEDIA_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Окно календаря: месяц, стрелки и возврат к сегодняшнему дню.
pub struct CalendarWin {
    frame: Frame,
    view: ContentView<CalendarPopup>,
    shown: Cell<(i32, u32)>,
    today: (i32, u32, u32),
    /// Накопленный сдвиг месяцев от кликов по стрелкам.
    step: Rc<Cell<i32>>,
    go_today: Rc<Cell<bool>>,
    /// Центр уведомлений: нажатия на «Не беспокоить» и на ссылку на родной центр Windows.
    dnd_clicked: Rc<Cell<bool>>,
    windows_clicked: Rc<Cell<bool>>,
    panel: bool,
}

impl CalendarWin {
    pub const WIDTH: f32 = 280.0;
    pub const HEIGHT: f32 = 292.0;

    pub const PANEL_HEIGHT: f32 = 452.0;

    pub fn open(anchor: Anchor, center_x: f32, now: &LocalTime) -> Result<CalendarWin> {
        Self::open_with(anchor, center_x, now, false)
    }

    /// Центр уведомлений: календарь, «Не беспокоить» и ссылка на родной центр Windows.
    pub fn open_panel(anchor: Anchor, center_x: f32, now: &LocalTime) -> Result<CalendarWin> {
        Self::open_with(anchor, center_x, now, true)
    }

    fn open_with(anchor: Anchor, center_x: f32, now: &LocalTime, panel: bool) -> Result<CalendarWin> {
        let height = if panel { Self::PANEL_HEIGHT } else { Self::HEIGHT };
        let frame = Frame::open_with_focus(anchor, center_x, Self::WIDTH, height, &popup_glass_params(), true)?;
        let view = ContentView::new_with(frame.size.0 as u32, frame.size.1 as u32, CalendarPopup::new);
        view.set_scale_factor(anchor.scale);
        let step = Rc::new(Cell::new(0));
        let slot = step.clone();
        view.ui.on_prev(move || slot.set(slot.get() - 1));
        let slot = step.clone();
        view.ui.on_next(move || slot.set(slot.get() + 1));
        let go_today = Rc::new(Cell::new(false));
        let slot = go_today.clone();
        view.ui.on_today(move || slot.set(true));
        let names: Vec<slint::SharedString> = calendar::WEEKDAYS.iter().map(|d| (*d).into()).collect();
        view.ui.set_weekdays(ModelRc::new(VecModel::from(names)));
        let today = (i32::from(now.year), u32::from(now.month), u32::from(now.day));
        view.ui.set_panel(panel);
        let dnd_clicked = Rc::new(Cell::new(false));
        let slot = dnd_clicked.clone();
        view.ui.on_dnd_toggled(move || slot.set(true));
        let windows_clicked = Rc::new(Cell::new(false));
        let slot = windows_clicked.clone();
        view.ui.on_windows_notifications(move || slot.set(true));
        let win = CalendarWin { frame, view, shown: Cell::new((today.0, today.1)), today, step, go_today, dnd_clicked, windows_clicked, panel };
        win.view.ui.set_dnd_on(crate::focus::is_on());
        win.render();
        win.frame.present(&win.view)?;
        win.frame.win.capture_mouse(true);
        win.frame.win.focus();
        Ok(win)
    }

    fn render(&self) {
        let (year, month) = self.shown.get();
        self.view.ui.set_month_title(calendar::title(year, month).into());
        self.view.ui.set_other_month((year, month) != (self.today.0, self.today.1));
        let days: Vec<CalDay> = calendar::month_grid(year, month, self.today)
            .into_iter()
            .map(|d| CalDay { text: d.day.to_string().into(), in_month: d.in_month, today: d.today })
            .collect();
        self.view.ui.set_days(ModelRc::new(VecModel::from(days)));
    }

    pub fn tick(&self) -> Result<bool> {
        let mut close = false;
        for ev in self.frame.win.take_input() {
            match ev {
                InputEvent::Key { vk, .. } => match vk {
                    0x25 => self.step.set(self.step.get() - 1),
                    0x27 => self.step.set(self.step.get() + 1),
                    0x24 => self.go_today.set(true),
                    0x1b => close = true,
                    _ => {}
                },
                InputEvent::Down { x, y } => {
                    close |= x < 0 || y < 0 || x >= self.frame.size.0 || y >= self.frame.size.1;
                    self.view.dispatch(&ev);
                }
                _ => self.view.dispatch(&ev),
            }
        }
        if self.dnd_clicked.replace(false) {
            crate::focus::set(!crate::focus::is_on());
        }
        if self.panel {
            self.view.ui.set_dnd_on(crate::focus::is_on());
        }
        if self.windows_clicked.replace(false) {
            // Родной центр уведомлений открывается сочетанием Win+N; помечено как своё, хук его не разбирает.
            crate::winkey::send_chord(&[(0x5B, false), (b'N' as u16, false)]);
            close = true;
        }
        let step = self.step.replace(0);
        let go_today = self.go_today.replace(false);
        if go_today {
            self.shown.set((self.today.0, self.today.1));
            self.render();
        } else if step != 0 {
            let (year, month) = self.shown.get();
            self.shown.set(calendar::shift_month(year, month, step));
            self.render();
        }
        self.frame.present(&self.view)?;
        Ok(close)
    }

    pub fn animating(&self) -> bool {
        self.view.animating()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bar_style::crop;
    use crate::slint_view::TopBar;

    #[test]
    fn keyboard_focus_separates_radio_details_from_switches() {
        assert_eq!(tile_keyboard_action(0), Some(CcAction::WifiExpand));
        assert_eq!(tile_keyboard_action(1), Some(CcAction::Wifi));
        assert_eq!(tile_keyboard_action(2), Some(CcAction::BtExpand));
        assert_eq!(tile_keyboard_action(3), Some(CcAction::Bluetooth));
    }

    /// Цветной фон под стеклом: по нему видно, что стекло прозрачное.
    fn backdrop(w: u32, h: u32) -> Bitmap {
        let mut b = Bitmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (r, g, bl) = (60 + x * 120 / w, 90 + y * 80 / h, 170 - x * 60 / w);
                b.pixels[(y * w + x) as usize] = 0xFF00_0000 | (r << 16) | (g << 8) | bl;
            }
        }
        b
    }

    fn on_glass(content: &[u32], mut bg: Bitmap, params: &GlassParams) -> Bitmap {
        let mut out = cpu::render(&bg, &GlassParams { blur_radius: 0, ..*params });
        bg.pixels.clear();
        cpu::composite_over(&mut out, content);
        out
    }

    /// Картинки окон для просмотра глазами: `MACSHELL_DUMP_DIR=... cargo test -p macshell --lib dump_previews -- --ignored`.
    #[test]
    #[ignore]
    fn dump_previews() {
        let dir = std::env::var("MACSHELL_DUMP_DIR").expect("MACSHELL_DUMP_DIR");
        crate::slint_view::set_accent([0x8a, 0x8f, 0x9b]);
        let write = |name: &str, b: &Bitmap| crate::snapshot::write_bmp(&format!("{dir}\\{name}.bmp"), b).unwrap();

        let dev = |id: &str, name: &str, default| AudioDevice { id: id.into(), name: name.into(), default };
        let lines = sound_lines(
            &[dev("a", "Динамики (Realtek Audio)", true), dev("b", "Наушники Bluetooth WH-1000XM5 Stereo", false)],
            &[dev("m", "Микрофон (Realtek Audio)", true)],
        );
        let h = sound_height(&lines).ceil() as u32;
        let view = ContentView::new_with(240, h, VolumePopup::new);
        view.ui.set_level(0.62);
        let rows: Vec<SoundRow> = lines
            .iter()
            .map(|l| SoundRow { text: l.text.as_str().into(), header: l.action == SoundAction::Header, current: l.current })
            .collect();
        view.ui.set_rows(ModelRc::new(VecModel::from(rows)));
        write("volume", &on_glass(&view.render(), backdrop(240, h), &popup_glass_params()));

        let view = ContentView::new_with(680, 420, crate::slint_view::Spotlight::new);
        let tabs: Vec<crate::slint_view::TabButton> = crate::spotlight::TAB_BUTTONS
            .iter()
            .map(|(n, h)| crate::slint_view::TabButton { name: (*n).into(), hint: (*h).into() })
            .collect();
        view.ui.set_tab_buttons(ModelRc::new(VecModel::from(tabs)));
        view.ui.set_active_tab(2);
        view.ui.set_tabs_visible(true);
        let line = |title: &str, kind: &str, glyph: &str, index: i32| crate::slint_view::SpotlightLine {
            header: false,
            gap: false,
            title: title.into(),
            kind: kind.into(),
            glyph: glyph.into(),
            icon: Default::default(),
            has_icon: false,
            index,
        };
        let header = crate::slint_view::SpotlightLine { header: true, title: "Действия".into(), index: -1, ..line("", "", "", -1) };
        let rows = vec![
            header,
            line("Заблокировать экран", "Действие", "lock", 0),
            line("Сон", "Действие", "power", 1),
            line("Переключить тёмную тему", "Действие", "moon", 2),
            line("Включить или выключить Bluetooth", "Действие", "bluetooth", 3),
            line("Снимок экрана", "Действие", "camera", 4),
        ];
        view.ui.set_lines(ModelRc::new(VecModel::from(rows)));
        view.ui.set_selected(1);
        let params = GlassParams { radius: 30.0, bevel: 24.0, refraction: 16.0, tint: [0.05, 0.06, 0.09, 0.36], ..popup_glass_params() };
        let content = view.render();
        let panel = (view.ui.get_panel_height().ceil() as u32).min(420);
        let mut shot = on_glass(&content, backdrop(680, 420), &params);
        shot.pixels.truncate((680 * panel) as usize);
        shot.height = panel;
        write("spotlight_tabs", &shot);

        view.ui.set_tabs_visible(true);
        view.ui.set_active_tab(1);
        view.ui.set_preview_title("План проекта.md".into());
        view.ui.set_preview_meta("MD · 4,2 КБ · изменён 3 ч назад".into());
        view.ui.set_preview_text("# План проекта\n\n- Купить домен\n- Нарисовать логотип\n- Собрать первую страницу".into());
        view.ui.set_preview_open(true);
        let content = view.render();
        let panel = (view.ui.get_panel_height().ceil() as u32).min(420);
        let mut shot = on_glass(&content, backdrop(680, 420), &params);
        shot.pixels.truncate((680 * panel) as usize);
        shot.height = panel;
        write("spotlight_preview", &shot);

        let cc_h = cc_height(true) as u32;
        let view = ContentView::new_with(316, cc_h, ControlCenter::new);
        view.ui.set_has_brightness(true);
        view.ui.set_brightness(0.7);
        view.ui.set_focus_on(true);
        view.ui.set_media_active(true);
        view.ui.set_media_playing(true);
        view.ui.set_media_title("Time".into());
        view.ui.set_media_artist("Pink Floyd".into());
        view.ui.set_wifi_subtitle(network_caption(1).into());
        view.ui.set_bluetooth_on(true);
        view.ui.set_bluetooth_subtitle(bluetooth_caption(crate::bluetooth::ON).into());
        view.ui.set_dark_on(true);
        view.ui.set_level(0.62);
        write("controlcenter", &on_glass(&view.render(), backdrop(316, cc_h), &popup_glass_params()));
        view.ui.set_keyboard_focus(0);
        write("controlcenter_wifi_details_focus", &on_glass(&view.render(), backdrop(316, cc_h), &popup_glass_params()));
        view.ui.set_keyboard_focus(1);
        write("controlcenter_wifi_toggle_focus", &on_glass(&view.render(), backdrop(316, cc_h), &popup_glass_params()));

        // Списки: Bluetooth отдельным окном, Wi-Fi страницей Центра управления, выключенное радио.
        let dev = |name: &str, connected, le| bluetooth::Device { name: name.into(), connected, address: 1, le };
        let list = BtList::with_devices(
            vec![
                dev("WH-1000XM5", true, false),
                dev("Logitech MX Master 3", true, false),
                dev("Xbox Wireless Controller", false, false),
                dev("JBL Flip 6", false, false),
                dev("Клавиатура K380", false, true),
            ],
            6,
        );
        let page = list.page();
        let rows = if page.on { page.rows.len() } else { 5 };
        let bt_h = lists::height(rows) as u32;
        let view = ContentView::new_with(316, bt_h, ControlCenter::new);
        let show = |view: &ContentView<ControlCenter>, kind: i32, page: Page, body: f32| {
            view.ui.set_standalone(true);
            view.ui.set_page(kind);
            view.ui.set_list_body_h(body);
            view.ui.set_list_title(page.title.into());
            view.ui.set_list_heading(page.heading.into());
            view.ui.set_list_on(page.on);
            view.ui.set_list_available(page.available);
            view.ui.set_list_state(page.state.into());
            view.ui.set_list_empty(page.empty.as_str().into());
            view.ui.set_list_footer(page.footer.as_str().into());
            view.ui.set_list_rows(ModelRc::new(VecModel::from(page.rows)));
        };
        let mut page = list.page();
        // Шапка и список на выключенном радио: `page()` читает живое состояние, для картинки берём включённое.
        page.on = true;
        page.state = "Включён";
        page.rows = list.devices.iter().take(5).map(|d| {
            let (status, action) = lists::bt_texts(d, None);
            crate::slint_view::ListRow { name: d.name.as_str().into(), status: status.into(), connected: d.connected, kind: bluetooth::kind_of(&d.name), action: action.into() }
        }).collect();
        show(&view, 1, page, lists::body(5));
        write("bluetooth", &on_glass(&view.render(), backdrop(316, bt_h), &popup_glass_params()));

        let net = |ssid: &str, signal, secured, connected, known| crate::wifi::WifiNet { ssid: ssid.into(), profile: ssid.into(), signal, secured, connected, known };
        let nets = [net("Home 5G", 92, true, true, true), net("Cafe Free", 64, false, false, false), net("Neighbour", 41, true, false, false), net("Office", 38, true, false, true)];
        let wifi_page = Page {
            title: "Wi-Fi",
            heading: "Сети",
            on: true,
            available: true,
            state: "Включён",
            rows: nets.iter().map(|n| {
                let (status, action) = lists::wifi_texts(n, false);
                crate::slint_view::ListRow { name: n.ssid.as_str().into(), status: status.as_str().into(), connected: n.connected, kind: if n.secured { 9 } else { 8 }, action: action.into() }
            }).collect(),
            empty: String::new(),
            footer: "Параметры сети…".into(),
        };
        let wifi_h = lists::height(4) as u32;
        let view = ContentView::new_with(316, wifi_h, ControlCenter::new);
        show(&view, 2, wifi_page, lists::body(4));
        write("wifi", &on_glass(&view.render(), backdrop(316, wifi_h), &popup_glass_params()));

        let view = ContentView::new_with(316, cc_h, ControlCenter::new);
        view.ui.set_has_brightness(true);
        let off = Page { title: "Bluetooth", heading: "Устройства", on: false, available: true, state: "Выключен", rows: Vec::new(), empty: "Включите Bluetooth, чтобы увидеть устройства".into(), footer: "Параметры Bluetooth…".into() };
        show(&view, 1, off, cc_h as f32 - 144.0);
        view.ui.set_standalone(false);
        write("bluetooth_off", &on_glass(&view.render(), backdrop(316, cc_h), &popup_glass_params()));

        let view = ContentView::new_with(280, 292, CalendarPopup::new);
        let days: Vec<CalDay> = calendar::month_grid(2026, 11, (2026, 11, 3))
            .into_iter()
            .map(|d| CalDay { text: d.day.to_string().into(), in_month: d.in_month, today: d.today })
            .collect();
        view.ui.set_days(ModelRc::new(VecModel::from(days)));
        view.ui.set_month_title(calendar::title(2026, 11).into());
        let names: Vec<slint::SharedString> = calendar::WEEKDAYS.iter().map(|d| (*d).into()).collect();
        view.ui.set_weekdays(ModelRc::new(VecModel::from(names)));
        write("calendar", &on_glass(&view.render(), backdrop(280, 292), &popup_glass_params()));

        let view = ContentView::new_with(820, 600, crate::slint_view::Apps::new);
        let chips: Vec<slint::SharedString> = crate::apps::CHIPS.iter().map(|c| (*c).into()).collect();
        view.ui.set_chips(ModelRc::new(VecModel::from(chips)));
        let names = ["Chrome", "Discord", "Blender 5.2", "Microsoft Edge", "Paint", "Telegram", "Steam", "OBS Studio", "Docker Desktop", "Claude", "Notepad", "Параметры", "Калькулятор"];
        let tiles: Vec<crate::slint_view::AppTile> =
            names.iter().map(|n| crate::slint_view::AppTile { name: (*n).into(), icon: Default::default(), has_icon: false }).collect();
        view.ui.set_tiles(ModelRc::new(VecModel::from(tiles)));
        let params = GlassParams { radius: 36.0, bevel: 26.0, refraction: 16.0, tint: [0.05, 0.06, 0.09, 0.42], ..popup_glass_params() };
        write("apps", &on_glass(&view.render(), backdrop(820, 600), &params));

        let view = ContentView::new_with(900, 102, crate::slint_view::DockView::new);
        let slot = |kind: i32, running: bool| crate::slint_view::DockSlot { icon: Default::default(), has_icon: false, running, kind, name: "Приложение".into() };
        let slots = vec![slot(1, false), slot(2, false), slot(0, true), slot(0, false), slot(0, true), slot(2, false), slot(3, false), slot(4, false)];
        view.ui.set_slots(ModelRc::new(VecModel::from(slots)));
        let content = view.render();
        let (pill_x, pill_w) = (view.ui.get_pill_x() as u32, view.ui.get_pill_width() as u32);
        let mut dock_bg = backdrop(900, 102);
        let pill_glass = cpu::render(&crop(&dock_bg, pill_x, 30, pill_w, 72), &GlassParams { radius: 28.0, blur_radius: 0, ..popup_glass_params() });
        for row in 0..72usize {
            let at = (row + 30) * 900 + pill_x as usize;
            dock_bg.pixels[at..at + pill_w as usize].copy_from_slice(&pill_glass.pixels[row * pill_w as usize..(row + 1) * pill_w as usize]);
        }
        cpu::composite_over(&mut dock_bg, &content);
        write("dock", &dock_bg);
        view.ui.set_hover_index(2);
        let mut hover = backdrop(900, 102);
        cpu::composite_over(&mut hover, &view.render());
        write("dock_hover", &hover);

        let view = ContentView::new_with(900, 24, TopBar::new);
        view.ui.set_clock("Вт 3 нояб. 14:05".into());
        view.ui.set_app_name("Блокнот".into());
        let titles: Vec<slint::SharedString> = ["Файл", "Правка", "Формат", "Вид", "Справка"].iter().map(|t| (*t).into()).collect();
        view.ui.set_app_menus(ModelRc::new(VecModel::from(titles)));
        view.ui.set_mic_active(true);
        view.ui.set_camera_active(true);
        view.ui.set_vpn_active(true);
        view.ui.set_input_lang("RU".into());
        view.ui.set_bluetooth(2);
        view.ui.set_battery(80);
        view.ui.set_battery_percent(true);
        view.ui.set_volume(2);
        view.ui.set_wifi(1);
        let mut bar = backdrop(900, 24);
        cpu::composite_over(&mut bar, &view.render());
        write("topbar", &bar);

        // Виджеты: плита и содержимое поверх фона.
        let (cw, ch) = (220u32, 150u32);
        let clock = ContentView::new_with(cw, ch, crate::slint_view::ClockWidget::new);
        clock.ui.set_time("20:31".into());
        clock.ui.set_date("Вторник, 3 ноября".into());
        let system = ContentView::new_with(cw, ch, crate::slint_view::SystemWidget::new);
        system.ui.set_cpu(0.34);
        system.ui.set_cpu_text("34%".into());
        system.ui.set_memory_percent("61%".into());
        system.ui.set_memory(0.61);
        system.ui.set_memory_text("9,7 из 15,9 ГБ".into());
        let mut sheet = backdrop(cw * 2 + 30, ch + 20);
        for (i, content) in [clock.render(), system.render()].into_iter().enumerate() {
            let behind = crop(&sheet, 10 + i as u32 * (cw + 10), 10, cw, ch);
            let one = on_glass(&content, behind, &crate::widgets::glass_params(1.0));
            for row in 0..ch as usize {
                let at = (10 + row) * sheet.width as usize + 10 + i * (cw as usize + 10);
                sheet.pixels[at..at + cw as usize].copy_from_slice(&one.pixels[row * cw as usize..(row + 1) * cw as usize]);
            }
        }
        write("widgets", &sheet);

        // Остальные виджеты: погода, что играет, диски, курсы.
        let weather = ContentView::new_with(cw, ch, crate::slint_view::WeatherWidget::new);
        weather.ui.set_loaded(true);
        weather.ui.set_city("Москва".into());
        weather.ui.set_temp("5°".into());
        weather.ui.set_condition("Дождь".into());
        weather.ui.set_range("Макс 6°, мин -2°, ощущается 1°".into());
        weather.ui.set_sky(4);
        let now = ContentView::new_with(cw, ch, crate::slint_view::NowPlayingWidget::new);
        now.ui.set_active(true);
        now.ui.set_playing(true);
        now.ui.set_track("Time".into());
        now.ui.set_artist("Pink Floyd".into());
        let storage = ContentView::new_with(cw, ch, crate::slint_view::StorageWidget::new);
        let drive = |name: &str, caption: &str, value: f32| crate::slint_view::DriveInfo { name: name.into(), caption: caption.into(), value };
        storage.ui.set_drives(ModelRc::new(VecModel::from(vec![drive("C:", "112 ГБ свободно", 0.77), drive("D:", "1,2 ТБ свободно", 0.35), drive("E:", "20 ГБ свободно", 0.96)])));
        let rates = ContentView::new_with(cw, ch, crate::slint_view::CurrencyWidget::new);
        let rate = |code: &str, value: &str| crate::slint_view::RateRow { code: code.into(), value: value.into() };
        rates.ui.set_rows(ModelRc::new(VecModel::from(vec![rate("USD", "80,12 ₽"), rate("EUR", "92,40 ₽"), rate("CNY", "11,25 ₽")])));
        rates.ui.set_caption("Курс ЦБ · 03.11".into());
        let mut sheet = backdrop(cw * 4 + 50, ch + 20);
        for (i, content) in [weather.render(), now.render(), storage.render(), rates.render()].into_iter().enumerate() {
            let behind = crop(&sheet, 10 + i as u32 * (cw + 10), 10, cw, ch);
            let one = on_glass(&content, behind, &crate::widgets::glass_params(1.0));
            for row in 0..ch as usize {
                let at = (10 + row) * sheet.width as usize + 10 + i * (cw as usize + 10);
                sheet.pixels[at..at + cw as usize].copy_from_slice(&one.pixels[row * cw as usize..(row + 1) * cw as usize]);
            }
        }
        write("widgets2", &sheet);

        // Настройки: две вкладки, текущий графитовый акцент.
        crate::slint_view::set_accent([0x8a, 0x8f, 0x9b]);
        for (tab, name) in [(4usize, "settings_keys"), (5, "settings_colors")] {
            let cfg = crate::config::Config::default();
            let view = ContentView::new_with(720, 500, crate::slint_view::Settings::new);
            view.ui.set_tabs(ModelRc::new(VecModel::from(crate::settings::TABS.iter().map(|t| slint::SharedString::from(*t)).collect::<Vec<_>>())));
            view.ui.set_tab(tab as i32);
            view.ui.set_note("Изменения применяются, когда окно закрыто: macshell перезапустится сам.".into());
            let rows = crate::settings::rows(tab, &cfg);
            let to = |r: &crate::settings::Row| {
                let strs = |v: &[String]| ModelRc::new(VecModel::from(v.iter().map(|x| slint::SharedString::from(x.as_str())).collect::<Vec<_>>()));
                let base = crate::slint_view::SetRow {
                    label: r.label.as_str().into(),
                    hint: r.hint.as_str().into(),
                    kind: 3,
                    value: 0,
                    options: ModelRc::new(VecModel::<slint::SharedString>::default()),
                    colors: ModelRc::new(VecModel::<slint::Color>::default()),
                };
                match &r.control {
                    crate::settings::Control::Heading => base,
                    crate::settings::Control::Toggle(on) => crate::slint_view::SetRow { kind: 0, value: i32::from(*on), ..base },
                    crate::settings::Control::Cycle(o, sel) => crate::slint_view::SetRow { kind: 1, value: *sel as i32, options: strs(o), ..base },
                    crate::settings::Control::Slider(value) => crate::slint_view::SetRow { kind: 4, value: *value as i32, ..base },
                    crate::settings::Control::Swatches(c, sel) => crate::slint_view::SetRow {
                        kind: 2,
                        value: sel.map_or(-1, |v| v as i32),
                        colors: ModelRc::new(VecModel::from(c.iter().map(|x| slint::Color::from_rgb_u8(x[0], x[1], x[2])).collect::<Vec<_>>())),
                        ..base
                    },
                }
            };
            view.ui.set_rows(ModelRc::new(VecModel::from(rows.iter().map(to).collect::<Vec<_>>())));
            if tab == 5 {
                if let Some(image) = crate::settings::glass_preview(40) {
                    view.ui.set_glass_preview(image);
                    view.ui.set_preview_available(true);
                }
            }
            write(name, &on_glass(&view.render(), backdrop(720, 500), &popup_glass_params()));
            if tab == 4 {
                view.ui.set_choice_row(5);
                view.ui.set_choice_index(0);
                write("settings_corner_choices", &on_glass(&view.render(), backdrop(720, 500), &popup_glass_params()));
            }
        }

        // Центр уведомлений (календарь с карточками) и заметки.
        let view = ContentView::new_with(280, 452, CalendarPopup::new);
        let days: Vec<CalDay> = calendar::month_grid(2026, 11, (2026, 11, 3))
            .into_iter()
            .map(|d| CalDay { text: d.day.to_string().into(), in_month: d.in_month, today: d.today })
            .collect();
        view.ui.set_days(ModelRc::new(VecModel::from(days)));
        view.ui.set_month_title(calendar::title(2026, 11).into());
        let names: Vec<slint::SharedString> = calendar::WEEKDAYS.iter().map(|d| (*d).into()).collect();
        view.ui.set_weekdays(ModelRc::new(VecModel::from(names)));
        view.ui.set_panel(true);
        view.ui.set_dnd_on(true);
        write("panel", &on_glass(&view.render(), backdrop(280, 452), &popup_glass_params()));

        let notes = ContentView::new_with(cw, ch, crate::slint_view::NotesWidget::new);
        notes.ui.set_note("Купить молоко\nПозвонить маме\nСобрать виджеты".into());
        notes.ui.set_editing(true);
        let one = on_glass(&notes.render(), backdrop(cw, ch), &crate::widgets::glass_params(1.0));
        write("notes", &one);

        // Mission Control с придуманными плитками и Stage Manager.
        let (mw, mh) = (1280u32, 720u32);
        let mission = ContentView::new_with(mw, mh, crate::slint_view::MissionControl::new);
        let area = (48.0, 120.0, mw as f32 - 96.0, mh as f32 - 120.0 - 40.0);
        let titles = ["Проводник", "Firefox", "Claude", "Блокнот", "Telegram"];
        let tiles: Vec<crate::slint_view::MissionTile> = titles
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (x, y, w, h) = crate::mission::tile_rect(i, titles.len(), area, 1.6);
                crate::slint_view::MissionTile { image: Default::default(), has_image: false, icon: Default::default(), has_icon: false, title: (*t).into(), x, y, w, h }
            })
            .collect();
        mission.ui.set_tiles(ModelRc::new(VecModel::from(tiles)));
        let desks = vec![crate::slint_view::DesktopTile { name: "Рабочий стол 1".into(), current: true }, crate::slint_view::DesktopTile { name: "Рабочий стол 2".into(), current: false }];
        mission.ui.set_desktops(ModelRc::new(VecModel::from(desks)));
        let mut mbg = backdrop(mw, mh);
        for px in mbg.pixels.iter_mut() {
            let c = |v: u32| v * 11 / 25;
            *px = 0xFF00_0000 | (c((*px >> 16) & 0xFF) << 16) | (c((*px >> 8) & 0xFF) << 8) | c(*px & 0xFF);
        }
        cpu::composite_over(&mut mbg, &mission.render());
        write("mission", &mbg);

        let strip = ContentView::new_with(136, 400, crate::slint_view::StageStrip::new);
        let st = |t: &str| crate::slint_view::StageTile { image: Default::default(), has_image: false, icon: Default::default(), has_icon: false, title: t.into() };
        strip.ui.set_tiles(ModelRc::new(VecModel::from(vec![st("a"), st("b"), st("c")])));
        let mut sbg = backdrop(136, 400);
        cpu::composite_over(&mut sbg, &strip.render());
        write("stage", &sbg);
    }

    #[test]
    fn sound_lines_group_outputs_and_inputs_and_end_with_settings() {
        let dev = |id: &str, default| AudioDevice { id: id.into(), name: format!("name-{id}"), default };
        let lines = sound_lines(&[dev("a", true), dev("b", false)], &[dev("m", true)]);
        let kinds: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(kinds, vec!["Вывод", "name-a", "name-b", "Ввод", "name-m", "Параметры звука…"]);
        assert_eq!(lines[1].action, SoundAction::Device("a".into()));
        assert!(lines[1].current && !lines[2].current);
        assert_eq!(lines.last().unwrap().action, SoundAction::Settings);
        // Без микрофонов раздела «Ввод» нет; без устройств остаётся только ссылка.
        assert_eq!(sound_lines(&[dev("a", true)], &[]).len(), 3);
        assert_eq!(sound_lines(&[], &[]).len(), 1);
    }

    #[test]
    fn sound_window_height_adds_slider_headers_rows_and_padding() {
        let dev = |id: &str| AudioDevice { id: id.into(), name: id.into(), default: false };
        let lines = sound_lines(&[dev("a")], &[dev("m")]);
        // 56 + 2 заголовка по 24 + 2 устройства и ссылка по 28 + 8.
        assert_eq!(sound_height(&lines), 56.0 + 48.0 + 84.0 + 8.0);
    }

    #[test]
    #[ignore = "opens real windows on the desktop"]
    fn list_windows_open_tick_and_switch_pages() {
        let anchor = Anchor { bar: RectI { x: 0, y: 0, w: 1920, h: 27 }, bar_height: 27, scale: 1.0 };
        for page in [CcPage::Bluetooth, CcPage::Wifi] {
            let win = ControlCenterWin::open_list(anchor, 900.0, 1, page).expect("open list window");
            for _ in 0..3 {
                let (close, open) = win.tick().expect("tick");
                assert!(!close && open.is_none());
            }
        }
        let win = ControlCenterWin::open(anchor, 900.0, 1).expect("open control center");
        win.actions.borrow_mut().push(CcAction::WifiExpand);
        win.tick().expect("tick");
        assert_eq!(win.page.get(), CcPage::Wifi);
        win.actions.borrow_mut().push(CcAction::ListBack);
        win.tick().expect("tick");
        assert_eq!(win.page.get(), CcPage::Tiles);
        win.actions.borrow_mut().push(CcAction::BtExpand);
        win.tick().expect("tick");
        assert_eq!(win.page.get(), CcPage::Bluetooth);
    }

    #[test]
    fn list_window_height_matches_the_panel() {
        for rows in [0usize, 1, 3, 6] {
            let view = ContentView::new_with(316, 500, ControlCenter::new);
            view.ui.set_standalone(true);
            view.ui.set_page(1);
            view.ui.set_list_on(true);
            view.ui.set_list_body_h(lists::body(rows));
            let row = |n: usize| crate::slint_view::ListRow { name: format!("d{n}").into(), status: "".into(), connected: false, kind: 0, action: "".into() };
            view.ui.set_list_rows(ModelRc::new(VecModel::from((0..rows).map(row).collect::<Vec<_>>())));
            assert_eq!(view.ui.get_list_height(), lists::height(rows), "{rows} rows");
        }
    }

    #[test]
    fn list_pages_fit_the_control_center() {
        assert_eq!(lists::max_rows(cc_height(false) - 144.0), 5);
        assert_eq!(lists::max_rows(cc_height(true) - 144.0), 6);
    }

    #[test]
    fn captions_follow_the_radio_and_network_state() {
        assert_eq!(bluetooth_caption(crate::bluetooth::ON), "Вкл.");
        assert_eq!(bluetooth_caption(crate::bluetooth::OFF), "Выкл.");
        assert_eq!(bluetooth_caption(crate::bluetooth::NONE), "Нет радио");
        assert_eq!([network_caption(0), network_caption(1), network_caption(2)], ["Нет сети", "Подключено", "Проводная сеть"]);
    }

    #[test]
    fn only_link_tiles_open_a_settings_page() {
        for a in [CcAction::Night, CcAction::Display, CcAction::Settings, CcAction::Cast] {
            assert!(cc_target(a).is_some_and(|t| t.starts_with("ms-")), "{a:?}");
        }
        assert!([CcAction::Wifi, CcAction::BtExpand, CcAction::WifiExpand, CcAction::ListToggle, CcAction::ListBack].iter().all(|a| cc_target(*a).is_none()));
        assert!(cc_target(CcAction::Bluetooth).is_none() && cc_target(CcAction::Dark).is_none() && cc_target(CcAction::Focus).is_none());
    }

    #[test]
    fn window_is_centred_under_the_icon() {
        assert_eq!(place_x(1000, 240, 1920), 880);
    }

    #[test]
    fn window_stays_on_the_screen_at_both_edges() {
        assert_eq!(place_x(10, 240, 1920), 0);
        assert_eq!(place_x(1915, 240, 1920), 1680);
    }

    #[test]
    fn window_wider_than_the_screen_sits_at_zero() {
        assert_eq!(place_x(100, 400, 300), 0);
    }
}
