//! Верхняя полоса: окно, системная панель, стекло и обновление по событиям.
//! Единственный таймер: один раз на границе минуты (часы).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};
use std::rc::Rc;

use anyhow::Result;
use capture::{RectI, capture_region};
use glass::{Bitmap, GlassParams, cpu};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetSystemMetrics, PostMessageW, RegisterWindowMessageW, SM_CXSCREEN, SM_CYSCREEN, WM_APP, WM_CLOSE,
    WM_DISPLAYCHANGE, WM_POWERBROADCAST, WM_SETTINGCHANGE, WM_TIMECHANGE,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use slint::{Image, ModelRc, VecModel};

use crate::anim::{FRAME_MS, ease_out_cubic, progress};
use crate::appbar::AppBar;
use crate::bar_style::{
    Ink, choose_ink, column_slice, crop, dock_overlapped, padded_rect, relative_luminance, scale_premultiplied, soft_blur, solid_background,
};
use crate::clock::{LocalTime, format_clock_with, ms_until_next_tick};
use crate::config::Config;
use crate::appmenu;
use crate::baritems;
use crate::secondary::{self, SecondaryBar};
use crate::widgets::{self, WidgetWin};
use crate::cover;
use crate::focus;
use crate::privacy;
use crate::dialogs::pick_file;
use crate::foreground::resource_name;
use crate::icons::{IconPixels, hicon_to_pixels_hq, load_icon, to_mono};
use crate::apps::AppsWin;
use crate::config::DockMode;
use crate::dock::{self, DockItem};
use crate::dockwin::{DockEvent, DockWin};
use crate::taskbar::HiddenTaskbar;
use crate::menu::{
    self, ID_ABOUT, ID_BAR_AUTOHIDE, ID_BAR_ITEM, ID_BAR_RESET, ID_BAR_SOLID, ID_ITEM_HIDE, ID_ITEM_LEFT, ID_ITEM_RIGHT, ID_APP_HIDE, ID_APP_HIDE_OTHERS, ID_APP_QUIT, ID_APPMENU_ITEM, ID_CLOCK_12H, ID_CLOCK_DATE, ID_CLOCK_SECONDS, ID_CLOCK_WEEKDAY, ID_PRIVACY_CAMERA, ID_PRIVACY_MIC, ID_BATTERY_PERCENT, ID_BATTERY_SAVER, ID_BATTERY_SETTINGS, ID_QUICK_KEYS, ID_FORCE_QUIT, ID_LOCK, ID_LOGOFF, ID_RECENT, ID_RESTART_PC, ID_SHUTDOWN, ID_SLEEP, ID_STORE,
    ID_SYSTEM_SETTINGS, ID_DOCK_AUTOHIDE, ID_DOCK_CLOSE, ID_DOCK_FIXED, ID_DOCK_PIN, ID_DOCK_SMART, ID_ADD_APP, ID_APPS, ID_PICK_LOGO, ID_QUIT, ID_REMOVE_PIN, ID_RESET_LOGO, ID_RESTART,
    ID_TRAY_STYLE,
};
use crate::host::{InputEvent, LayeredWindow, primary_scale, pump_messages, set_dpi_awareness};
use crate::files::FileEntry;
use crate::index::{Entry, Usage, build_index, settings_entries};
use crate::spotlight::{Launch, Outcome, SpotlightWin, set_clipboard_text};
use crate::winkey::{self, WinKey};
use crate::slint_view::{ContentView, Menu, MenuEntry, PinItem, TopBar, TrayFlyout, TrayItem};
use crate::tray::{first_line, flyout_size};
use crate::tray::fallback::{FallbackApp, activate_app, fallback_apps, file_name_lower};
use crate::tray::host::{Button, TrayHost};
use crate::tray::state::TrayIcon;
use crate::snapshot::write_bmp;
use crate::corners::Corners;
use crate::mission::{self, MissionWin};
use crate::desktop;
use crate::settings::SettingsWin;
use crate::stage::Stage;
use crate::switcher::{self, Mouse, SwitcherWin};
use crate::popups::{Anchor, CalendarWin, CcPage, ControlCenterWin, VolumeWin};
use crate::power::{self, PowerAction};
use crate::{battery, bluetooth, foreground, lang, status};

/// Высота панели равна высоте чёлки Island при его наименьшем масштабе (80 %, меньше он не делается): 27 px.
const HEIGHT_PT: f32 = 27.0;
const MSG_APPBAR: u32 = WM_APP + 1;
/// Автоскрытие панели: пауза после ухода мыши, первая пауза после запуска и толщина полоски у верхнего края.
const AUTOHIDE_DELAY_MS: u64 = 600;
const AUTOHIDE_FIRST_MS: u64 = 2000;
const TOP_STRIP_PX: i32 = 2;
const MSG_FOREGROUND: u32 = WM_APP + 2;
/// Сколько пунктов показывает меню окна приложения: длинные меню не должны уходить за экран.
const MAX_APP_ITEMS: usize = 26;
const APP_MORE_WIDTH: f32 = 52.0;

fn visible_menu_count(titles: &[String], start: f32, end: f32) -> usize {
    // Широкие буквы (например W) заметно шире среднего символа; оставляем запас до Island.
    let widths: Vec<f32> = titles.iter().map(|t| t.chars().count() as f32 * 13.0 + 16.0).collect();
    if start + widths.iter().sum::<f32>() <= end { return titles.len(); }
    let mut used = start + APP_MORE_WIDTH;
    widths.into_iter().take_while(|w| { used += *w; used <= end }).count()
}
const MSG_STATUS: u32 = WM_APP + 3;
const MSG_TRAY: u32 = WM_APP + 4;
const MSG_COVER: u32 = WM_APP + 5;
/// Короткое нажатие Win (хук `winkey`) и готовый индекс Spotlight (фоновый поток).
const MSG_WINKEY: u32 = WM_APP + 6;
const MSG_INDEX: u32 = WM_APP + 7;
const MSG_DOCK_APPBAR: u32 = WM_APP + 8;
/// Сообщения оболочки (`HSHELL_*`), после которых набор окон в Dock мог измениться.
const SHELL_WINDOW_EVENTS: [usize; 4] = [1, 2, 4, 0x8004];
/// Пауза после события оболочки: окна появляются пачками, список читается один раз.
const DOCK_DEBOUNCE: Duration = Duration::from_millis(60);
/// Индекс приложений старше этого перестраивается при следующем открытии Spotlight.
const INDEX_MAX_AGE: Duration = Duration::from_secs(300);
const FILES_MAX_AGE: Duration = Duration::from_secs(600);

/// Для проверки снаружи (тесты, отладка): сейчас ли панель в состоянии «окно закрывает рабочий стол».
static COVERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_covered_state() -> bool {
    COVERED.load(std::sync::atomic::Ordering::Relaxed)
}

const ABN_POSCHANGED: usize = 1;
const ABN_FULLSCREENAPP: usize = 2;
const SPI_SETDESKWALLPAPER: usize = 0x0014;
const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
const PBT_APMRESUMESUSPEND: usize = 0x0007;
const PBT_APMPOWERSTATUSCHANGE: usize = 0x000A;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
const WM_DPICHANGED: u32 = 0x02E0;

/// Сообщения, после которых полоса собирается заново: новое разрешение или новый масштаб основного монитора.
/// Без `WM_DPICHANGED` смена 100% на 125% оставляла полосу в старом размере до перезапуска.
fn rebuilds_on(msg: u32) -> bool {
    msg == WM_DISPLAYCHANGE || msg == WM_DPICHANGED
}

struct Bar {
    // Порядок важен: сначала снимается панель, потом закрывается окно.
    appbar: AppBar,
    /// Сколько пикселей панель занимает у верхнего края: высота панели, при автоскрытии ноль.
    reserve: i32,
    /// Невидимая полоска у верхнего края: ловит мышь, пока панель спрятана (только при автоскрытии).
    ah_strip: Option<LayeredWindow>,
    ah_shown: Cell<bool>,
    ah_leave_at: Cell<Option<Instant>>,
    /// Скольжение панели: начало, смещение от, смещение до (пиксели по вертикали).
    ah_slide: Cell<Option<(Instant, i32, i32)>>,
    win: LayeredWindow,
    view: ContentView<TopBar>,
    /// Сплошной фон для состояния «окно закрывает рабочий стол». На рабочем столе панель прозрачна.
    solid: Bitmap,
    covered: Cell<bool>,
    /// Насколько виден сплошной фон: 0 прозрачно, 1 сплошной; переход плавный.
    mix: Cell<f32>,
    mix_anim: Cell<Option<(Instant, f32, f32)>>,
    mix_dirty: Cell<bool>,
    /// Цвет значков по обоям под левой и правой частью (на рабочем столе).
    wallpaper_ink: Cell<(Ink, Ink)>,
    /// Однократная перепроверка обоев после старта: пока панель занимает своё место, под ней ещё могут быть окна.
    recheck_backdrop_at: Cell<Option<Instant>>,
    scale: f32,
    height: i32,
    /// Центр логотипа в логических пикселях, пока клик не обработан.
    menu_request: Rc<Cell<Option<f32>>>,
    /// Значок приложения, по которому кликнули (номер в `cfg.pins`).
    pin_click: Rc<Cell<Option<usize>>>,
    /// Правый клик по значку приложения: номер и центр в логических пикселях.
    pin_menu_request: Rc<Cell<Option<(usize, f32)>>>,
    /// Клики по стрелке трея и по значкам трея на панели.
    tray_chevron: Rc<Cell<Option<f32>>>,
    tray_click: Rc<Cell<Option<usize>>>,
    tray_right: Rc<Cell<Option<usize>>>,
    /// Клики по звуку, Bluetooth и часам (центр значка в логических пикселях) и по раскладке.
    volume_request: Rc<Cell<Option<f32>>>,
    bluetooth_request: Rc<Cell<Option<f32>>>,
    clock_request: Rc<Cell<Option<f32>>>,
    control_request: Rc<Cell<Option<f32>>>,
    wifi_request: Rc<Cell<Option<f32>>>,
    battery_request: Rc<Cell<Option<f32>>>,
    privacy_request: Rc<Cell<Option<f32>>>,
    focus_request: Rc<Cell<Option<f32>>>,
    item_menu_request: Rc<Cell<Option<(i32, f32)>>>,
    /// Значок утащили: вид, смещение и ширина (логические пиксели).
    item_drag_request: Rc<Cell<Option<(i32, f32, f32)>>>,
    bar_menu_request: Rc<Cell<Option<f32>>>,
    vpn_request: Rc<Cell<bool>>,
    lang_request: Rc<Cell<bool>>,
    /// Клик по значку без своего окна: Spotlight, сеть, батарея.
    quick_request: Rc<Cell<QuickItem>>,
    /// Значки трея, которые сейчас показаны на панели (по ним расшифровываются номера кликов).
    pinned_icons: RefCell<Vec<TrayIcon>>,
    /// Название приложения на панели и начало его появления (прозрачность и сдвиг).
    shown_name: RefCell<String>,
    shown_clock: RefCell<String>,
    /// Час и минута, на которых часы показывались в последний раз (`часы * 60 + минуты`).
    shown_minute: Cell<u32>,
    clock_menu_request: Rc<Cell<Option<f32>>>,
    /// Окно приложения, чьё меню и название сейчас в полосе (0, когда впереди рабочий стол).
    app_target: Cell<isize>,
    app_name_request: Rc<Cell<Option<f32>>>,
    app_menu_request: Rc<Cell<Option<(i32, f32)>>>,
    app_menu_more: Cell<Option<usize>>,
    name_anim: Cell<Option<Instant>>,
    /// Подпись показанного: пока она та же, панель не перерисовывается.
    tray_sig: RefCell<Vec<(isize, u32, isize)>>,
    /// Картинки значков под размер панели, по (окно, номер, дескриптор значка).
    tray_cache: RefCell<HashMap<(isize, u32, isize), Image>>,
    cfg: Config,
}

fn slint_image(icon: &IconPixels) -> Image {
    crate::slint_view::image_from_pixels(icon)
}

/// Запуск закреплённого приложения средствами оболочки (как двойной клик в Проводнике).
/// Значки правой части панели, у которых нет собственного окна.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuickItem {
    None,
    Spotlight,
}

pub(crate) fn launch(path: &str) {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name).to_string()
}

/// Панели на других мониторах (при автоскрытии их нет). Та, что не открылась, пропускается.
fn open_other_bars(bar: &Bar) -> Vec<SecondaryBar> {
    if bar.cfg.bar_autohide {
        return Vec::new();
    }
    secondary::other_monitors()
        .into_iter()
        .filter_map(|m| opened("secondary bar", SecondaryBar::open(m, bar.height, bar.scale, bar.cfg.bar_color)))
        .collect()
}

/// Пересоздаёт виджет после смены размера или цвета: окно другого размера не растянуть.
fn reopen_widget(list: &mut Vec<WidgetWin>, kind: usize, cfg: &Config) {
    list.retain(|w| w.kind != kind);
    if let Some(spec) = widgets::entries(&cfg.widgets).into_iter().find(|s| s.kind == kind) {
        if let Some(w) = opened("widget", WidgetWin::open(spec, &cfg.weather)) {
            list.push(w);
        }
    }
}

/// Виджеты из конфига; тот, что не открылся, пропускается.
fn open_widgets(cfg: &Config) -> Vec<WidgetWin> {
    widgets::entries(&cfg.widgets).into_iter().filter_map(|spec| opened("widget", WidgetWin::open(spec, &cfg.weather))).collect()
}

/// Невидимая полоска вдоль верхнего края экрана для автоскрытия. Один процент прозрачности, чтобы окно получало
/// мышь (совсем прозрачные точки мышь пропускают).
fn top_strip(width: i32) -> Result<LayeredWindow> {
    let strip = LayeredWindow::new(RectI { x: 0, y: -50, w: width, h: TOP_STRIP_PX })?;
    let mut bmp = Bitmap::new(width as u32, TOP_STRIP_PX as u32);
    bmp.pixels.fill(0x0100_0000);
    strip.present(&bmp)?;
    Ok(strip)
}

impl Bar {
    fn build() -> Result<Bar> {
        let scale = primary_scale();
        let height = (HEIGHT_PT * scale).round() as i32;
        let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let win = LayeredWindow::new(RectI { x: 0, y: 0, w: width, h: height })?;
        win.set_title("macshell.bar");
        let autohide = Config::load().bar_autohide;
        let reserve = if autohide { 0 } else { height };
        let appbar = AppBar::register(win.hwnd(), MSG_APPBAR, reserve)?;
        let ah_strip = if autohide { top_strip(width).ok() } else { None };
        let r = appbar.rect();
        win.set_position(r.x, r.y)?;

        trace("window+appbar ready");
        let view = ContentView::new_with(width as u32, height as u32, TopBar::new);
        trace("slint view ready");
        view.set_scale_factor(scale);
        let menu_request = Rc::new(Cell::new(None));
        let slot = menu_request.clone();
        view.ui.on_logo_clicked(move |x| slot.set(Some(x)));
        let pin_click = Rc::new(Cell::new(None));
        let slot = pin_click.clone();
        view.ui.on_pin_clicked(move |i| slot.set(Some(i as usize)));
        let pin_menu_request = Rc::new(Cell::new(None));
        let slot = pin_menu_request.clone();
        view.ui.on_pin_menu(move |i, x| slot.set(Some((i as usize, x))));

        let tray_chevron = Rc::new(Cell::new(None));
        let slot = tray_chevron.clone();
        view.ui.on_tray_clicked(move |x| slot.set(Some(x)));
        let tray_click = Rc::new(Cell::new(None));
        let slot = tray_click.clone();
        view.ui.on_tray_pin_clicked(move |i| slot.set(Some(i as usize)));
        let tray_right = Rc::new(Cell::new(None));
        let slot = tray_right.clone();
        view.ui.on_tray_pin_menu(move |i, _| slot.set(Some(i as usize)));

        let volume_request = Rc::new(Cell::new(None));
        let slot = volume_request.clone();
        view.ui.on_volume_clicked(move |x| slot.set(Some(x)));
        let bluetooth_request = Rc::new(Cell::new(None));
        let slot = bluetooth_request.clone();
        view.ui.on_bluetooth_clicked(move |x| slot.set(Some(x)));
        let clock_request = Rc::new(Cell::new(None));
        let slot = clock_request.clone();
        view.ui.on_clock_clicked(move |x| slot.set(Some(x)));
        let app_name_request = Rc::new(Cell::new(None));
        let slot = app_name_request.clone();
        view.ui.on_app_name_clicked(move |x| slot.set(Some(x)));
        let app_menu_request = Rc::new(Cell::new(None));
        let slot = app_menu_request.clone();
        view.ui.on_app_menu_clicked(move |i, x| slot.set(Some((i, x))));
        let clock_menu_request = Rc::new(Cell::new(None));
        let slot = clock_menu_request.clone();
        view.ui.on_clock_menu(move |x| slot.set(Some(x)));
        let control_request = Rc::new(Cell::new(None));
        let slot = control_request.clone();
        view.ui.on_control_clicked(move |x| slot.set(Some(x)));
        let lang_request = Rc::new(Cell::new(false));
        let slot = lang_request.clone();
        view.ui.on_lang_clicked(move || slot.set(true));
        let quick_request = Rc::new(Cell::new(QuickItem::None));
        let slot = quick_request.clone();
        view.ui.on_spotlight_clicked(move || slot.set(QuickItem::Spotlight));
        let wifi_request = Rc::new(Cell::new(None));
        let slot = wifi_request.clone();
        view.ui.on_wifi_clicked(move |x| slot.set(Some(x)));
        let battery_request = Rc::new(Cell::new(None));
        let slot = battery_request.clone();
        view.ui.on_battery_clicked(move |x| slot.set(Some(x)));
        let item_menu_request = Rc::new(Cell::new(None));
        let slot = item_menu_request.clone();
        view.ui.on_item_menu(move |k, x| slot.set(Some((k, x))));
        let item_drag_request = Rc::new(Cell::new(None));
        let slot = item_drag_request.clone();
        view.ui.on_item_dragged(move |k, dx, unit| slot.set(Some((k, dx, unit))));
        let bar_menu_request = Rc::new(Cell::new(None));
        let slot = bar_menu_request.clone();
        view.ui.on_bar_menu(move |x| slot.set(Some(x)));
        let focus_request = Rc::new(Cell::new(None));
        let slot = focus_request.clone();
        view.ui.on_focus_clicked(move |x| slot.set(Some(x)));
        let privacy_request = Rc::new(Cell::new(None));
        let slot = privacy_request.clone();
        view.ui.on_privacy_clicked(move |x| slot.set(Some(x)));
        let vpn_request = Rc::new(Cell::new(false));
        let slot = vpn_request.clone();
        view.ui.on_vpn_clicked(move |_| slot.set(true));

        let cfg = Config::load();
        let mut bar = Bar {
            appbar,
            reserve,
            ah_strip,
            ah_shown: Cell::new(true),
            ah_leave_at: Cell::new(autohide.then(|| Instant::now() + Duration::from_millis(AUTOHIDE_FIRST_MS))),
            ah_slide: Cell::new(None),
            win,
            view,
            solid: solid_background(width as u32, height as u32, cfg.bar_color),
            covered: Cell::new(false),
            mix: Cell::new(0.0),
            mix_anim: Cell::new(None),
            mix_dirty: Cell::new(true),
            wallpaper_ink: Cell::new((Ink::White, Ink::White)),
            recheck_backdrop_at: Cell::new(Some(Instant::now() + std::time::Duration::from_millis(800))),
            scale,
            height,
            menu_request,
            pin_click,
            pin_menu_request,
            tray_chevron,
            tray_click,
            tray_right,
            volume_request,
            bluetooth_request,
            clock_request,
            control_request,
            wifi_request,
            battery_request,
            privacy_request,
            focus_request,
            item_menu_request,
            item_drag_request,
            bar_menu_request,
            vpn_request,
            lang_request,
            quick_request,
            pinned_icons: RefCell::new(Vec::new()),
            shown_name: RefCell::new(String::new()),
            shown_clock: RefCell::new(String::new()),
            shown_minute: Cell::new(u32::MAX),
            clock_menu_request,
            app_target: Cell::new(0),
            app_name_request,
            app_menu_request,
            app_menu_more: Cell::new(None),
            name_anim: Cell::new(None),
            tray_sig: RefCell::new(Vec::new()),
            tray_cache: RefCell::new(HashMap::new()),
            cfg,
        };
        bar.apply_config();
        bar.view.ui.set_tray_enabled(!bar.cfg.tray_off);
        bar.view.ui.set_tray_mono(bar.cfg.tray_mono);
        bar.view.ui.set_battery_percent(bar.cfg.battery_percent);
        bar.refresh_cover(false);
        bar.refresh_backdrop()?;
        trace("backdrop+glass ready");
        bar.refresh_clock();
        bar.refresh_app();
        trace("clock+app ready");
        bar.refresh_status();
        bar.refresh_lang(None);
        trace("status ready");
        Ok(bar)
    }

    /// Свой логотип и значки закреплённых приложений из конфига. Нечитаемый значок не выбрасывается из
    /// конфига: пустое место на панели, правый клик убирает его.
    fn apply_config(&self) {
        let px = self.tray_px();
        match self.cfg.logo.as_deref().and_then(|p| load_icon(p, px)) {
            Some(icon) => {
                self.view.ui.set_logo(slint_image(&icon));
                self.view.ui.set_has_logo(true);
            }
            None => self.view.ui.set_has_logo(false),
        }
        let pins: Vec<PinItem> = self
            .cfg
            .pins
            .iter()
            .map(|p| PinItem {
                icon: load_icon(p, px).map(|i| slint_image(&i)).unwrap_or_default(),
                name: file_stem(p).into(),
            })
            .collect();
        self.view.ui.set_pins(ModelRc::new(VecModel::from(pins)));
    }

    /// Размер значка на панели (физические пиксели): ровно такой, каким он показывается (16 логических).
    /// Значки рисуются сразу в этом размере с фильтром Ланцоша, и Slint их второй раз не масштабирует.
    fn tray_px(&self) -> i32 {
        (16.0 * self.scale).round() as i32
    }

    /// Название приложения для подписи: из ресурсов exe, иначе первая строка подсказки.
    fn tray_name(host: &TrayHost, icon: &TrayIcon) -> String {
        host.exe_of(icon).and_then(|exe| resource_name(&exe)).unwrap_or_else(|| first_line(&icon.tip))
    }

    /// Картинка значка трея в стиле панели: цветная или одноцветная.
    fn tray_image(&self, icon: &TrayIcon, px: i32) -> Image {
        hicon_to_pixels_hq(icon.hicon, px as u32)
            .map(|p| if self.cfg.tray_mono { to_mono(&p) } else { p })
            .map(|p| slint_image(&p))
            .unwrap_or_default()
    }

    /// Живые значки трея на самой панели: те, чьё приложение закреплено. Перерисовка только при смене значков.
    fn refresh_tray(&self, host: &TrayHost) {
        let pinned: Vec<TrayIcon> =
            host.icons().into_iter().filter(|i| host.exe_of(i).is_some_and(|e| self.cfg.is_tray_pinned(&e))).collect();
        let sig: Vec<(isize, u32, isize)> = pinned.iter().map(|i| (i.hwnd, i.uid, i.hicon)).collect();
        if *self.tray_sig.borrow() == sig {
            return;
        }
        let px = self.tray_px();
        let mut cache = self.tray_cache.borrow_mut();
        let items: Vec<TrayItem> = pinned
            .iter()
            .map(|icon| TrayItem {
                icon: cache.entry((icon.hwnd, icon.uid, icon.hicon)).or_insert_with(|| self.tray_image(icon, px)).clone(),
                limited: false,
                name: Self::tray_name(host, icon).into(),
                tip: first_line(&icon.tip).into(),
                pinned: true,
                hidden: false,
            })
            .collect();
        cache.retain(|k, _| sig.contains(k));
        self.view.ui.set_tray_pins(ModelRc::new(VecModel::from(items)));
        *self.pinned_icons.borrow_mut() = pinned;
        *self.tray_sig.borrow_mut() = sig;
    }

    /// Стиль значков сменился или конфиг изменён: пересобрать значки трея на панели.
    fn reset_tray(&self, host: Option<&TrayHost>) {
        self.tray_cache.borrow_mut().clear();
        self.tray_sig.borrow_mut().clear();
        if let Some(h) = host {
            self.refresh_tray(h);
        }
    }

    /// Читает обои под панелью и выбирает цвет значков для левой и правой части (чёрный или белый).
    fn refresh_backdrop(&mut self) -> Result<()> {
        let backdrop = capture_region(self.win.rect())?;
        let (previous_left, previous_right) = self.wallpaper_ink.get();
        let (ll, lr) = (relative_luminance(&column_slice(&backdrop, 0.0, 0.25)), relative_luminance(&column_slice(&backdrop, 0.55, 1.0)));
        trace(&format!("wallpaper luminance left {ll:.3} right {lr:.3}"));
        let left = choose_ink(ll, previous_left);
        let right = choose_ink(lr, previous_right);
        self.wallpaper_ink.set((left, right));
        self.apply_ink();
        Ok(())
    }

    /// Цвет значков и текста: белый на сплошном фоне, на рабочем столе по обоям. Меняется плавно (Slint).
    fn apply_ink(&self) {
        let (left, right) = if self.covered.get() { (Ink::White, Ink::White) } else { self.wallpaper_ink.get() };
        trace(&format!("ink left {left:?} right {right:?} (covered {})", self.covered.get()));
        let color = |ink: Ink| match ink {
            Ink::White => slint::Color::from_argb_u8(0xEB, 255, 255, 255),
            Ink::Black => slint::Color::from_argb_u8(0xE6, 20, 20, 26),
        };
        self.view.ui.set_ink_left_to(color(left));
        self.view.ui.set_ink_right_to(color(right));
    }

    /// Пересчитывает «закрывает ли окно рабочий стол». `animate`: фон меняется плавно, иначе сразу.
    /// `true`, если состояние изменилось.
    fn refresh_cover(&self, animate: bool) -> bool {
        // Пока виден рабочий стол, панель прозрачная; окно приложения на весь экран делает её сплошной, цвета
        // чёлки Island. `bar = "solid"` держит её сплошной всегда.
        let covered = self.cfg.bar_solid || cover::is_desktop_covered();
        if covered == self.covered.get() {
            return false;
        }
        self.covered.set(covered);
        COVERED.store(covered, std::sync::atomic::Ordering::Relaxed);
        trace(&format!("cover: {covered}"));
        let to = if covered { 1.0 } else { 0.0 };
        if animate && crate::anim::enabled() {
            self.mix_anim.set(Some((Instant::now(), self.mix.get(), to)));
        } else {
            self.mix.set(to);
            self.mix_anim.set(None);
            self.mix_dirty.set(true);
        }
        self.apply_ink();
        true
    }

    /// Кадр перехода сплошного фона. `true`, пока он идёт.
    fn tick_mix(&self) -> bool {
        let Some((start, from, to)) = self.mix_anim.get() else { return false };
        let t = progress(start, 220);
        self.mix.set(from + (to - from) * ease_out_cubic(t));
        self.mix_dirty.set(true);
        if t >= 1.0 {
            self.mix_anim.set(None);
        }
        t < 1.0
    }

    /// Кадр панели: фон (прозрачный, сплошной или между ними) и поверх него значки и текст.
    fn compose(&self, content: &[u32]) -> Bitmap {
        let mix = self.mix.get();
        let mut out = Bitmap::new(self.solid.width, self.solid.height);
        if mix >= 0.999 {
            out.pixels.copy_from_slice(&self.solid.pixels);
        } else if mix > 0.001 {
            scale_premultiplied(&self.solid.pixels, mix, &mut out.pixels);
        }
        cpu::composite_over(&mut out, content);
        out
    }

    /// Чёлка Island стоит на верхнем краю: в центре панели остаётся место под неё.
    fn refresh_island(&self) {
        let half = match crate::island::find() {
            Some(w) if crate::island::sits_in_bar(&w, self.height) => {
                // Чёлка должна быть видна поверх панели, даже если панель запущена позже Island.
                self.win.place_below(w.hwnd);
                crate::island::NOTCH_PT / 2.0
            }
            _ => 0.0,
        };
        if self.view.ui.get_notch_half() != half {
            self.view.ui.set_notch_half(half);
            self.refresh_app_menus(self.app_target.get(), &self.shown_name.borrow());
        }
    }

    /// `true`, если началась новая минута (секунды, если их показывают, этого не считают).
    fn refresh_clock(&self) -> bool {
        let now = LocalTime::now();
        let text = format_clock_with(&now, &self.cfg.clock);
        if *self.shown_clock.borrow() == text {
            return false;
        }
        *self.shown_clock.borrow_mut() = text.clone();
        self.view.ui.set_clock(text.into());
        let minute = now.hour as u32 * 60 + now.minute as u32;
        self.shown_minute.replace(minute) != minute
    }

    /// Названия меню окна в полосе: столько, сколько помещается между названием приложения и чёлкой Island.
    fn refresh_app_menus(&self, target: isize, name: &str) {
        let titles = if target == 0 { Vec::new() } else { appmenu::titles(target) };
        let bar_w = self.win.rect().w as f32 / self.scale;
        let notch = self.view.ui.get_notch_half();
        let end = if notch > 0.0 { bar_w / 2.0 - notch - 12.0 } else { bar_w * 0.45 };
        let name_width = (name.chars().count() as f32 * 10.0).min((end - 56.0 - APP_MORE_WIDTH).max(0.0));
        self.view.ui.set_app_name_limit(name_width);
        let count = visible_menu_count(&titles, 56.0 + name_width, end);
        let mut shown: Vec<slint::SharedString> = titles.iter().take(count).map(|t| t.as_str().into()).collect();
        self.app_menu_more.set((count < titles.len()).then_some(count));
        if count < titles.len() { shown.push("Ещё".into()); }
        self.view.ui.set_app_menus(slint::ModelRc::new(slint::VecModel::from(shown)));
    }

    /// Новое название: старое сменяется плавно (прозрачность и сдвиг). Первое показывается сразу.
    fn refresh_app(&self) {
        // Spotlight впереди: приложение под ним не меняется, название остаётся прежним.
        if foreground::foreground_is_own() {
            return;
        }
        let target = appmenu::front_window().unwrap_or(0);
        self.app_target.set(target);
        let name = foreground::current();
        self.refresh_app_menus(target, &name);
        let first = self.shown_name.borrow().is_empty();
        if *self.shown_name.borrow() == name {
            return;
        }
        *self.shown_name.borrow_mut() = name.clone();
        self.view.ui.set_app_name(name.into());
        if !first && crate::anim::enabled() {
            self.view.ui.set_name_opacity(0.0);
            self.view.ui.set_name_offset(-8.0);
            self.name_anim.set(Some(Instant::now()));
        } else {
            self.name_anim.set(None);
            self.view.ui.set_name_opacity(1.0);
            self.view.ui.set_name_offset(0.0);
        }
    }

    /// Кадр анимации названия. `true`, пока она идёт.
    fn tick_name(&self) -> bool {
        let Some(start) = self.name_anim.get() else { return false };
        let t = progress(start, 220);
        let e = ease_out_cubic(t);
        self.view.ui.set_name_opacity(e);
        self.view.ui.set_name_offset(-8.0 * (1.0 - e));
        if t >= 1.0 {
            self.name_anim.set(None);
        }
        t < 1.0
    }

    /// Автоскрытие: панель выезжает, когда мышь у верхнего края, и уезжает, когда мышь ушла и ничего не открыто.
    /// `hold`: открыто всплывающее окно или Spotlight, панель не прячется. `true`, пока идёт скольжение.
    fn tick_autohide(&self, hold: bool) -> bool {
        if !self.cfg.bar_autohide {
            return false;
        }
        if let Some(strip) = &self.ah_strip {
            let touched = strip.take_input().iter().any(|e| matches!(e, InputEvent::Move { .. } | InputEvent::Down { .. }));
            if touched && !self.ah_shown.get() {
                self.ah_set(true);
            }
        }
        if self.ah_shown.get() {
            let mut at = windows::Win32::Foundation::POINT::default();
            let near = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut at) }.is_ok() && at.y <= self.height + 2;
            if near || hold {
                self.ah_leave_at.set(Some(Instant::now() + Duration::from_millis(AUTOHIDE_DELAY_MS)));
            } else if self.ah_leave_at.get().is_some_and(|t| Instant::now() >= t) {
                self.ah_set(false);
            }
        }
        let Some((start, from, to)) = self.ah_slide.get() else { return false };
        let t = ease_out_cubic(progress(start, 200));
        let dy = from + ((to - from) as f32 * t).round() as i32;
        let _ = self.win.set_blend(255, dy);
        if t >= 1.0 {
            self.ah_slide.set(None);
        }
        t < 1.0
    }

    /// Показывает или прячет панель со скольжением; полоска у края работает, пока панель спрятана.
    fn ah_set(&self, show: bool) {
        self.ah_shown.set(show);
        let from = self.win.blend().1;
        let to = if show { 0 } else { -(self.height + 2) };
        if crate::anim::enabled() {
            self.ah_slide.set(Some((Instant::now(), from, to)));
        } else {
            self.ah_slide.set(None);
            let _ = self.win.set_blend(255, to);
        }
        self.ah_leave_at.set(show.then(|| Instant::now() + Duration::from_millis(AUTOHIDE_DELAY_MS * 3)));
        if let Some(strip) = &self.ah_strip {
            let _ = strip.set_position(0, if show { -50 } else { 0 });
        }
    }

    /// Через сколько миллисекунд нужен шаг автоскрытия без событий (опрос мыши, пока панель видна).
    fn ah_wake_ms(&self) -> u32 {
        if self.cfg.bar_autohide && self.ah_shown.get() { 120 } else { u32::MAX }
    }

    /// Какие значки состояния сейчас существуют: у компьютера без батареи нет значка батареи и так далее.
    fn active_kinds(&self) -> [bool; 10] {
        let ui = &self.view.ui;
        [
            ui.get_battery() >= 0,
            ui.get_focus_active(),
            ui.get_mic_active() || ui.get_camera_active(),
            ui.get_vpn_active(),
            ui.get_bluetooth() > 0,
            !ui.get_input_lang().is_empty(),
            true,
            true,
            true,
            true,
        ]
    }

    /// Какие значки видны: существующие и не скрытые пользователем.
    fn shown_kinds(&self) -> [bool; 10] {
        let hidden = baritems::parse_hidden(&self.cfg.bar_hidden);
        let mut shown = self.active_kinds();
        for k in hidden {
            shown[k] = false;
        }
        shown
    }

    /// Отдаёт панели список значков в порядке из конфига.
    fn refresh_items(&self) {
        let order = baritems::parse_order(&self.cfg.bar_order);
        let hidden = baritems::parse_hidden(&self.cfg.bar_hidden);
        let list = baritems::visible(&order, &hidden, &self.active_kinds());
        self.view.ui.set_status_order(slint::ModelRc::new(slint::VecModel::from(list)));
    }

    fn refresh_status(&self) {
        let s = status::read();
        self.view.ui.set_wifi(s.wifi);
        self.view.ui.set_volume(s.volume);
        self.view.ui.set_battery(s.battery);
        self.view.ui.set_charging(s.charging);
        self.view.ui.set_bluetooth(s.bluetooth);
        self.view.ui.set_mic_active(s.mic);
        self.view.ui.set_camera_active(s.camera);
        self.view.ui.set_vpn_active(s.vpn);
        self.view.ui.set_focus_active(s.focus);
        self.refresh_items();
    }

    /// Код раскладки окна в фокусе (`langid` берётся из сообщения оболочки, если оно пришло).
    fn refresh_lang(&self, langid: Option<u16>) {
        let code = lang::code_from_langid(langid.unwrap_or_else(lang::current_langid));
        self.view.ui.set_input_lang(code.into());
        self.refresh_items();
    }

    /// Где стоит полоса: от этого считается место всплывающих окон.
    fn anchor(&self) -> Anchor {
        Anchor { bar: self.win.rect(), bar_height: self.height, scale: self.scale }
    }

    /// Рисует и показывает кадр, если содержимое изменилось (или `force`).
    fn present(&self, force: bool) -> Result<()> {
        let changed = self.view.render_if_changed();
        let background_changed = self.mix_dirty.replace(false);
        if changed.is_none() && !background_changed && !force {
            return Ok(());
        }
        // Содержимое не менялось, а фон да: берём прошлый кадр содержимого.
        let content = changed.unwrap_or_else(|| self.view.render());
        self.win.present(&self.compose(&content))?;
        Ok(())
    }
}

/// Стекло меню: тёмное, радиус 14 (радиус строки 8 + отступ 6).
static GLASS_DENSITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(40);

pub(crate) fn set_glass_density(value: u8) {
    GLASS_DENSITY.store(value.clamp(10, 85), std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn menu_glass() -> GlassParams {
    GlassParams {
        radius: 14.0,
        bevel: 10.0,
        refraction: 8.0,
        blur_radius: 12,
        saturation: 1.5,
        tint: [0.05, 0.06, 0.09, GLASS_DENSITY.load(std::sync::atomic::Ordering::Relaxed) as f32 / 100.0],
        dispersion: 1.0,
        rim: 0.5,
        ..GlassParams::default()
    }
}

/// Стекло всплывающего окна в прямоугольнике `rect`: фон снимается с запасом вокруг окна, размывается мягко
/// (почти гауссово) и обрезается по окну, потом стекло рисуется уже без своего размытия.
pub(crate) fn popup_glass(rect: RectI, params: &GlassParams) -> Result<Bitmap> {
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let padded = padded_rect(rect, 2 * params.blur_radius as i32, sw, sh);
    let soft = soft_blur(&capture_region(padded)?, params.blur_radius);
    let inner = crop(&soft, (rect.x - padded.x) as u32, (rect.y - padded.y) as u32, rect.w as u32, rect.h as u32);
    Ok(cpu::render(&inner, &GlassParams { blur_radius: 0, ..*params }))
}

/// Окно меню логотипа. Пока оно открыто, мышь захвачена: клик вне окна закрывает его.
struct MenuWin {
    win: LayeredWindow,
    view: ContentView<Menu>,
    glass: Bitmap,
    size: (i32, i32),
    kind: MenuKind,
    /// `id` выбранного пункта или -1.
    picked: Rc<Cell<i32>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Logo,
    Battery,
    Privacy,
    Focus,
    Clock,
    /// Меню значка состояния и меню пустого места панели.
    Item,
    /// Меню названия приложения или одного из меню его окна.
    App,
    /// Меню значка с этим номером в `cfg.pins`.
    Pin(usize),
    /// Меню значка Dock.
    Dock,
}

impl MenuWin {
    /// Меню под панелью. `center_x`: центр по горизонтали в логических пикселях, `None` значит у левого края.
    fn open(bar: &Bar, entries: Vec<MenuEntry>, center_x: Option<f32>, kind: MenuKind) -> Result<MenuWin> {
        MenuWin::open_placed(bar, entries, center_x, kind, None)
    }

    /// Как `open`, но `above` (верх Dock в пикселях экрана) ставит меню над этой чертой, а не под полосой.
    fn open_placed(bar: &Bar, entries: Vec<MenuEntry>, center_x: Option<f32>, kind: MenuKind, above: Option<i32>) -> Result<MenuWin> {
        let s = bar.scale;
        let (w, h) = ((menu::WIDTH * s).round() as i32, (menu::height(&entries) * s).ceil() as i32);
        let screen_w = bar.win.rect().w;
        let x = match center_x {
            None => (8.0 * s).round() as i32,
            Some(cx) => ((cx * s) as i32 - w / 2).clamp(0, (screen_w - w).max(0)),
        };
        let y = match above {
            Some(top) => (top - h - (6.0 * s).round() as i32).max(bar.win.rect().y + bar.height),
            None => bar.win.rect().y + bar.height + (4.0 * s).round() as i32,
        };
        let mut win = LayeredWindow::new(RectI { x, y, w, h })?;
        win.animate_overlay(true, ((if above.is_some() { 6.0 } else { -6.0 }) * s).round() as i32);
        let view = ContentView::new_with(w as u32, h as u32, Menu::new);
        view.set_scale_factor(s);
        view.ui.set_entries(ModelRc::new(VecModel::from(entries)));
        let picked = Rc::new(Cell::new(-1));
        let slot = picked.clone();
        view.ui.on_activated(move |i| slot.set(i));
        let glass = popup_glass(win.rect(), &menu_glass().scaled(s))?;
        let menu = MenuWin { win, view, glass, size: (w, h), kind, picked };
        menu.present()?;
        menu.win.capture_mouse(true);
        Ok(menu)
    }

    fn present(&self) -> Result<()> {
        if let Some(content) = self.view.render_if_changed() {
            let mut out = self.glass.clone();
            cpu::composite_over(&mut out, &content);
            self.win.present(&out)?;
        }
        Ok(())
    }

    /// Обрабатывает мышь и Esc. `true`, если меню пора закрыть.
    fn handle_input(&self) -> bool {
        let mut close = false;
        for ev in self.win.take_input() {
            if let InputEvent::Down { x, y } = ev {
                close |= x < 0 || y < 0 || x >= self.size.0 || y >= self.size.1;
            }
            self.view.dispatch(&ev);
        }
        close || unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } as u16 & 0x8000 != 0
    }
}

impl Drop for MenuWin {
    fn drop(&mut self) {
        self.win.capture_mouse(false);
    }
}

/// Вторая копия при `--restart` ждёт, пока уйдёт первая; обычный запуск при второй копии просто сообщает.
#[derive(Debug)]
pub struct AlreadyRunning;

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("macshell is already running (use --quit to stop it)")
    }
}

impl std::error::Error for AlreadyRunning {}

enum FlyoutAction {
    Click(usize),
    Right(usize),
    Pin(usize),
    /// Убрать значок из окна (или вернуть, если показаны убранные).
    Hide(usize),
    /// Кнопка внизу: показать убранные значки или снова спрятать.
    Reveal,
}

/// Окно со всеми значками трея. Как меню: мышь захвачена, клик вне окна и Esc закрывают его.
struct FlyoutWin {
    win: LayeredWindow,
    view: ContentView<TrayFlyout>,
    glass: Bitmap,
    size: (i32, i32),
    /// Центр под стрелкой (логические пиксели): окно пересобирается на том же месте.
    center_x: f32,
    /// Показанные значки хоста в порядке плиток: по номеру плитки находим, кому слать клик.
    icons: Vec<TrayIcon>,
    /// Все значки хоста, в том числе убранные: из них пересобирается набор плиток.
    all: Vec<TrayIcon>,
    /// Приглушённые плитки после живых: приложения, оставшиеся в родном трее.
    fallbacks: Vec<FallbackApp>,
    /// Убранные значки тоже показаны (приглушённо, с кнопкой «вернуть»).
    show_hidden: bool,
    /// Подпись показанного набора: окно пересобирается, только когда она меняется.
    sig: Vec<(isize, u32, isize)>,
    actions: Rc<RefCell<Vec<FlyoutAction>>>,
}

impl FlyoutWin {
    fn open(bar: &Bar, host: &TrayHost, center_x: f32, show_hidden: bool) -> Result<FlyoutWin> {
        let s = bar.scale;
        let all = host.icons();
        // Убранное приложение не должно вернуться приглушённой плиткой: хостовые имена берутся из полного набора.
        let hosted: HashSet<String> = all.iter().filter_map(|i| host.exe_of(i)).map(|p| file_name_lower(&p)).collect();
        let fallbacks: Vec<FallbackApp> =
            fallback_apps(&hosted).into_iter().filter(|a| show_hidden || !bar.cfg.is_tray_hidden(&a.exe_path)).collect();
        let icons = Self::visible(bar, host, &all, show_hidden);
        let (lw, lh) = flyout_size(icons.len() + fallbacks.len());
        let (w, h) = ((lw * s).round() as i32, (lh * s).ceil() as i32);
        let screen_w = bar.win.rect().w;
        let x = ((center_x * s) as i32 - w / 2).clamp(0, (screen_w - w).max(0));
        let y = bar.win.rect().y + bar.height + (4.0 * s).round() as i32;
        let mut win = LayeredWindow::new(RectI { x, y, w, h })?;
        win.animate_overlay(true, (-6.0 * s).round() as i32);
        let view = ContentView::new_with(w as u32, h as u32, TrayFlyout::new);
        view.set_scale_factor(s);
        let actions = Rc::new(RefCell::new(Vec::new()));
        let a = actions.clone();
        view.ui.on_clicked(move |i| a.borrow_mut().push(FlyoutAction::Click(i as usize)));
        let a = actions.clone();
        view.ui.on_right_clicked(move |i| a.borrow_mut().push(FlyoutAction::Right(i as usize)));
        let a = actions.clone();
        view.ui.on_pin_toggled(move |i| a.borrow_mut().push(FlyoutAction::Pin(i as usize)));
        let a = actions.clone();
        view.ui.on_hide_toggled(move |i| a.borrow_mut().push(FlyoutAction::Hide(i as usize)));
        let a = actions.clone();
        view.ui.on_reveal_toggled(move || a.borrow_mut().push(FlyoutAction::Reveal));
        let glass = popup_glass(win.rect(), &flyout_glass().scaled(s))?;
        let mut flyout = FlyoutWin {
            win,
            view,
            glass,
            size: (w, h),
            center_x,
            icons: Vec::new(),
            all: Vec::new(),
            fallbacks,
            show_hidden,
            sig: Vec::new(),
            actions,
        };
        flyout.set_icons(bar, host, all);
        flyout.present()?;
        flyout.win.capture_mouse(true);
        Ok(flyout)
    }

    /// Значки, которые попадают в окно: убранные только в режиме «показать убранные».
    fn visible(bar: &Bar, host: &TrayHost, all: &[TrayIcon], show_hidden: bool) -> Vec<TrayIcon> {
        all.iter()
            .filter(|i| show_hidden || !host.exe_of(i).is_some_and(|e| bar.cfg.is_tray_hidden(&e)))
            .cloned()
            .collect()
    }

    /// Сколько плиток получится из `all`: от этого зависит размер окна.
    fn tile_count(&self, bar: &Bar, host: &TrayHost, all: &[TrayIcon]) -> usize {
        Self::visible(bar, host, all, self.show_hidden).len() + self.fallbacks.len()
    }

    /// Показывает значки `all` (кроме убранных); закреплённость и скрытость берутся из конфига.
    fn set_icons(&mut self, bar: &Bar, host: &TrayHost, all: Vec<TrayIcon>) {
        let px = (24.0 * bar.scale).round() as i32; // ровно размер показа: Slint не масштабирует второй раз
        let icons = Self::visible(bar, host, &all, self.show_hidden);
        let hidden_live = all.iter().filter(|i| host.exe_of(i).is_some_and(|e| bar.cfg.is_tray_hidden(&e))).count();
        // Приглушённые плитки убранных приложений в окно не попали, но в счётчике они есть.
        let hosted: HashSet<String> = all.iter().filter_map(|i| host.exe_of(i)).map(|p| file_name_lower(&p)).collect();
        let hidden_fallbacks = fallback_apps(&hosted).iter().filter(|a| bar.cfg.is_tray_hidden(&a.exe_path)).count();
        self.view.ui.set_hidden_count((hidden_live + hidden_fallbacks) as i32);
        self.view.ui.set_show_hidden(self.show_hidden);
        let live = icons.iter().map(|icon| TrayItem {
            limited: false,
            icon: bar.tray_image(icon, px),
            name: Bar::tray_name(host, icon).into(),
            tip: first_line(&icon.tip).into(),
            pinned: host.exe_of(icon).is_some_and(|e| bar.cfg.is_tray_pinned(&e)),
            hidden: host.exe_of(icon).is_some_and(|e| bar.cfg.is_tray_hidden(&e)),
        });
        let muted = self.fallbacks.iter().map(|app| {
            let image = load_icon(&app.exe_path, px)
                .map(|p| if bar.cfg.tray_mono { to_mono(&p) } else { p })
                .map(|p| slint_image(&p))
                .unwrap_or_default();
            let tip = if app.tip.is_empty() { "В родном трее: клик покажет окно".to_string() } else { first_line(&app.tip) };
            TrayItem {
                limited: true,
                icon: image,
                name: resource_name(&app.exe_path).unwrap_or_else(|| file_stem(&app.exe_path)).into(),
                tip: tip.into(),
                pinned: false,
                hidden: bar.cfg.is_tray_hidden(&app.exe_path),
            }
        });
        let items: Vec<TrayItem> = live.chain(muted).collect();
        self.view.ui.set_items(ModelRc::new(VecModel::from(items)));
        self.sig = icon_signature(&all);
        self.icons = icons;
        self.all = all;
    }

    fn present(&self) -> Result<()> {
        if let Some(content) = self.view.render_if_changed() {
            let mut out = self.glass.clone();
            cpu::composite_over(&mut out, &content);
            self.win.present(&out)?;
        }
        Ok(())
    }

    /// Мышь и Esc. `true`, если окно пора закрыть.
    fn handle_input(&self) -> bool {
        let mut close = false;
        for ev in self.win.take_input() {
            if let InputEvent::Down { x, y } = ev {
                close |= x < 0 || y < 0 || x >= self.size.0 || y >= self.size.1;
            }
            self.view.dispatch(&ev);
        }
        close || unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } as u16 & 0x8000 != 0
    }
}

impl Drop for FlyoutWin {
    fn drop(&mut self) {
        self.win.capture_mouse(false);
    }
}

/// Стекло окна значков: как у меню, радиус 20 (радиус плитки 12 + отступ 8).
fn flyout_glass() -> GlassParams {
    GlassParams { radius: 20.0, bevel: 14.0, refraction: 10.0, ..menu_glass() }
}

struct Instance(HANDLE);

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Вторая копия полосы заняла бы тот же край экрана: не даём ей запуститься.
fn single_instance() -> Result<Instance> {
    unsafe {
        let handle = CreateMutexW(None, false, w!("Local\\macshell.bar"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return Err(AlreadyRunning.into());
        }
        Ok(Instance(handle))
    }
}

/// Строит индекс Spotlight в фоне; готовый список приходит в канал, а окну полосы уходит `MSG_INDEX`.
fn spawn_index(hwnd: isize) -> Receiver<Vec<Entry>> {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new().name("index".into()).spawn(move || {
        let entries = build_index();
        if tx.send(entries).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(windows::Win32::Foundation::HWND(hwnd as *mut _)), MSG_INDEX, WPARAM(0), LPARAM(0));
            }
        }
    });
    rx
}

/// Исполняет выбор в Spotlight: запускает приложение или страницу настроек, либо копирует результат расчёта.
fn run_launch(what: &Launch, usage: &mut Usage) {
    match what {
        Launch::Entry(e) => {
            launch(&e.target);
            usage.bump(&e.target);
            let _ = usage.save();
        }
        Launch::Copy(text) => {
            set_clipboard_text(text);
        }
        Launch::File { path, reveal } => {
            if *reveal {
                // Проводник открывается с выделенным файлом.
                let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
            } else {
                launch(path);
            }
        }
        Launch::Action(id) => crate::actions::run(*id),
        Launch::Open(target) => launch(target),
    }
}

/// Строит индекс файлов для вкладки «Файлы» в фоне; готовый приходит в канал, а окну полосы уходит `MSG_INDEX`.
fn spawn_files(hwnd: isize) -> Receiver<Vec<FileEntry>> {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new().name("files".into()).spawn(move || {
        let files = crate::files::build();
        if tx.send(files).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(windows::Win32::Foundation::HWND(hwnd as *mut _)), MSG_INDEX, WPARAM(0), LPARAM(0));
            }
        }
    });
    rx
}

/// Просит запущенную полосу выйти. `false`, если её нет.
pub fn request_quit() -> bool {
    unsafe {
        match FindWindowW(w!("macshell_layered"), w!("macshell.bar")) {
            Ok(hwnd) if !hwnd.0.is_null() => PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok(),
            _ => false,
        }
    }
}

/// Один кадр полосы вместе с фоном под ней в BMP: окна приложения исключены из захвата экрана.
pub fn snapshot(path: &str, covered: bool) -> Result<()> {
    set_dpi_awareness()?;
    let bar = Bar::build()?;
    std::thread::sleep(std::time::Duration::from_millis(800)); // панель заняла место: под ней остались только обои
    let mut bar = bar;
    bar.refresh_backdrop()?;
    // Состояние задаём вручную, чтобы увидеть оба вида независимо от того, что открыто на экране.
    bar.covered.set(covered);
    bar.mix.set(if covered { 1.0 } else { 0.0 });
    bar.apply_ink();
    bar.view.render();
    std::thread::sleep(std::time::Duration::from_millis(350)); // цвет значков меняется плавно
    let content = bar.view.render();
    let out = bar.compose(&content);
    let mut shot = capture_region(bar.win.rect())?;
    cpu::composite_over(&mut shot, &out.pixels);
    write_bmp(path, &shot)
}

pub fn run() -> Result<()> {
    crate::slint_view::set_accent(Config::load().accent.unwrap_or([0x46, 0x73, 0xE2]));
    set_glass_density(Config::load().glass_density.unwrap_or(40));
    trace("run start");
    let _instance = single_instance()?;
    set_dpi_awareness()?;
    let _window_drag = crate::window_drag::WindowDrag::start();
    if _window_drag.is_none() { trace("window drag hook unavailable"); }
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };

    let mut bar = Bar::build()?;
    crate::brightness::start();
    foreground::watch(bar.win.hwnd(), MSG_FOREGROUND);
    status::watch(bar.win.hwnd(), MSG_STATUS);
    cover::watch(bar.win.hwnd(), MSG_COVER);
    bluetooth::watch();
    let shell_msg = lang::register_shell_hook(bar.win.hwnd());
    trace("watchers ready");
    bar.refresh_island();
    crate::clipboard::listen(bar.win.hwnd());
    bar.present(true)?;
    trace("first frame presented");
    let mut menu: Option<MenuWin> = None;
    let mut flyout: Option<FlyoutWin> = None;
    let mut volume: Option<VolumeWin> = None;
    let mut switch_session: Option<SwitchSession> = None;
    let mut last_tap: Option<Instant> = None;
    let mut settings_win: Option<SettingsWin> = None;
    let mut mission_win: Option<MissionWin> = None;
    let mut stage: Option<Stage> = if bar.cfg.stage_on { Stage::open().ok() } else { None };
    let mut calendar: Option<CalendarWin> = None;
    let mut control: Option<ControlCenterWin> = None;
    // Сети, показанные в открытом меню Wi-Fi: по номеру пункта находим, к какой подключаться.
    // Окно и команды открытого меню приложения: выбранный пункт отправляется туда.
    let mut app_menu_target = 0isize;
    let mut app_menu_more_anchor = 0.0f32;
    let mut app_menu_more_first = 0usize;
    let mut app_menu_more_offset = 0usize;
    let mut app_cmds: Vec<u32> = Vec::new();
    let mut item_target = 0usize;
    widgets::align_legacy_triplet(&mut bar.cfg.widgets);
    let mut desk_widgets: Vec<WidgetWin> = open_widgets(&bar.cfg);
    let mut widget_target = 0usize;
    let mut other_bars: Vec<SecondaryBar> = open_other_bars(&bar);
    let mut corners: Option<Corners> = Corners::open(&bar.cfg.corners);
    let mut apps: Option<AppsWin> = None;
    let mut dock: Option<DockWin> = None;
    let mut hidden_taskbar: Option<HiddenTaskbar> = None;
    let mut dock_order: Vec<String> = Vec::new();
    let mut dock_dirty_at: Option<Instant> = None;
    // Элемент Dock, для которого открыто меню.
    let mut dock_menu_item: Option<DockItem> = None;
    // MACSHELL_NO_CLAIM (тесты) не трогает чужую панель задач.
    let dock_allowed = bar.cfg.dock != DockMode::Off
        && std::env::var_os("MACSHELL_NO_CLAIM").is_none()
        && std::env::var_os("MACSHELL_NO_DOCK").is_none();
    let mut spotlight: Option<SpotlightWin> = None;
    // Spotlight: настройки доступны сразу, приложения подгружаются в фоне. MACSHELL_NO_WINKEY (тесты) не ставит хук.
    let mut index: Arc<Vec<Entry>> = Arc::new(settings_entries());
    let mut index_rx = spawn_index(bar.win.hwnd());
    let mut index_built = Instant::now();
    // Индекс файлов (вкладка «Файлы»): строится при первой просьбе, потом обновляется не чаще, чем раз в 10 минут.
    let mut files: Option<Arc<Vec<FileEntry>>> = None;
    let mut files_rx: Option<Receiver<Vec<FileEntry>>> = None;
    let mut files_built = Instant::now();
    let mut usage = Usage::load();
    let _winkey: Option<WinKey> = if (bar.cfg.spotlight_off && bar.cfg.win_start_keep && bar.cfg.switcher_off) || std::env::var_os("MACSHELL_NO_WINKEY").is_some() {
        None
    } else {
        winkey::set_tap(!bar.cfg.win_start_keep);
        winkey::set_switcher(!bar.cfg.switcher_off);
        WinKey::start(bar.win.hwnd(), MSG_WINKEY).map_err(|e| trace(&format!("win key hook failed: {e}"))).ok()
    };
    let (mut iterations, mut counted_since) = (0u32, Instant::now());
    let started = std::time::Instant::now();
    // Трей: окно Shell_TrayWnd над Проводником; MACSHELL_NO_CLAIM (тесты) не трогает уже запущенные приложения.
    let tray: Option<TrayHost> = if bar.cfg.tray_off {
        None
    } else {
        Some(TrayHost::start(bar.win.hwnd(), MSG_TRAY, std::env::var_os("MACSHELL_NO_CLAIM").is_none())?)
    };
    if let Some(t) = &tray {
        bar.reset_tray(Some(t));
    }
    if dock_allowed {
        seed_dock(&mut bar.cfg);
        dock = opened("dock", DockWin::open(bar.cfg.dock, MSG_DOCK_APPBAR));
        if let Some(d) = dock.as_mut() {
            refresh_dock(d, &bar.cfg.dock_pins, &mut dock_order);
            d.present()?;
            sync_dock_overlap(d);
            // Родная панель прячется, только когда наш Dock уже на экране.
            hidden_taskbar = HiddenTaskbar::hide();
        }
    } else if bar.cfg.dock == DockMode::Off && std::env::var_os("MACSHELL_NO_CLAIM").is_none() {
        crate::taskbar::restore_from_file(); // Dock выключен в конфиге: прошлая авария не должна оставить панель спрятанной
    }

    loop {
        iterations += 1;
        if counted_since.elapsed().as_secs() >= 5 {
            trace(&format!("loop iterations in 5 s: {iterations}"));
            (iterations, counted_since) = (0, Instant::now());
        }
        let (mut quit, mut rebuild, mut recapture, mut tray_changed) = (false, false, false, false);
        let mut toggle_spotlight = false;
        let mut toggle_apps = false;
        let mut mission_request = false;
        let mut panel_request = false;
        let mut stage_request = false;
        let mut switch_cmds: Vec<usize> = Vec::new();
        if let Some(rx) = &files_rx {
            if let Ok(list) = rx.try_recv() {
                let list = Arc::new(list);
                files = Some(list.clone());
                files_rx = None;
                files_built = Instant::now();
                if let Some(s) = spotlight.as_mut() {
                    s.set_files(list);
                }
                crate::host::trim_working_set();
            }
        }
        while let Ok(entries) = index_rx.try_recv() {
            index = Arc::new(entries);
            index_built = Instant::now();
            crate::apps::warm(&index); // значки и стекло сетки готовятся заранее, чтобы открытие не задерживалось
            crate::host::trim_working_set(); // сборка индекса подняла библиотеки оболочки в память
            if let Some(s) = spotlight.as_mut() {
                s.set_index(index.clone());
            }
            if let Some(a) = apps.as_mut() {
                a.set_index(index.clone());
            }
        }
        for ev in bar.win.take_input() {
            let InputEvent::Message { msg, wparam, lparam } = ev else {
                bar.view.dispatch(&ev);
                continue;
            };
            match msg {
                MSG_APPBAR => match wparam {
                    ABN_FULLSCREENAPP => {
                        let covered = lparam != 0;
                        bar.appbar.set_fullscreen(covered);
                        if let Some(strip) = &bar.ah_strip {
                            strip.set_topmost(!covered);
                        }
                        if let Some(c) = &corners {
                            c.set_topmost(!covered);
                        }
                        // Сочетания отдаются системе только в игре с монопольным полноэкранным режимом. В обычном окне на весь
                        // экран они остаются нашими: иначе Win и Alt+Tab то работали бы, то нет, смотря какое окно впереди.
                        winkey::set_enabled(!(covered && crate::appbar::exclusive_fullscreen()));
                    }
                    ABN_POSCHANGED => {
                        if bar.appbar.reposition(bar.reserve)? {
                            let r = bar.appbar.rect();
                            bar.win.set_position(r.x, r.y)?;
                        }
                    }
                    _ => {}
                },
                MSG_FOREGROUND => {
                    if let Some(s) = stage.as_mut() {
                        let front = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize;
                        let _ = s.update(&crate::winlist::list(), front);
                    }
                    bar.refresh_cover(true);
                    bar.refresh_island();
                    if let Some(d) = dock.as_mut() {
                        sync_dock_overlap(d);
                    }
                    bar.refresh_app();
                    bar.refresh_lang(None);
                    if let Some(t) = &tray {
                        t.raise();
                    }
                }
                MSG_TRAY => tray_changed = true,
                MSG_WINKEY if wparam >= 10 => switch_cmds.push(wparam - 10),
                MSG_WINKEY if wparam == 1 => toggle_apps = true,
                MSG_WINKEY => toggle_spotlight = !bar.cfg.spotlight_off,
                MSG_COVER => {
                    cover::ack();
                    bar.refresh_cover(true);
                    if let Some(d) = dock.as_mut() {
                        sync_dock_overlap(d);
                    }
                }
                MSG_STATUS => {
                    bar.refresh_status();
                    if let Some(v) = &volume {
                        v.sync(); // громкость могли сменить кнопками клавиатуры
                    }
                }
                m if m == shell_msg => {
                    if wparam == lang::HSHELL_LANGUAGE {
                        bar.refresh_lang(Some(lang::langid_of_hkl(lparam)));
                    } else if SHELL_WINDOW_EVENTS.contains(&wparam) && dock.is_some() {
                        dock_dirty_at.get_or_insert(Instant::now() + DOCK_DEBOUNCE);
                        if let Some(h) = &hidden_taskbar {
                            h.enforce();
                        }
                    }
                }
                crate::clipboard::WM_CLIPBOARDUPDATE => crate::clipboard::capture(),
                WM_POWERBROADCAST => {
                    if matches!(wparam, PBT_APMPOWERSTATUSCHANGE | PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND) {
                        bar.refresh_status();
                        bar.refresh_clock();
                    }
                }
                WM_TIMECHANGE => {
                    bar.refresh_clock();
                }
                WM_SETTINGCHANGE => recapture |= wparam == SPI_SETDESKWALLPAPER,
                WM_DWMCOLORIZATIONCOLORCHANGED => recapture = true,
                m if rebuilds_on(m) => rebuild = true,
                WM_CLOSE => quit = true,
                // Свою рассылку при старте и выходе пропускаем; настоящий перезапуск Проводника приходит позже.
                m if m == taskbar_created => rebuild |= started.elapsed().as_secs() >= 5,
                _ => {}
            }
        }
        if quit {
            return Ok(());
        }
        if rebuild {
            // Новое разрешение, DPI или перезапуск Проводника: полоса собирается заново.
            menu = None;
            flyout = None;
            volume = None;
            calendar = None;
            control = None;
            apps = None;
            dock = None;
            desk_widgets.clear();
            other_bars.clear();
            drop(bar);
            bar = Bar::build()?;
            desk_widgets = open_widgets(&bar.cfg);
            other_bars = open_other_bars(&bar);
            corners = Corners::open(&bar.cfg.corners);
            // Новое окно полосы: источники событий шлют теперь ему.
            winkey::set_target(bar.win.hwnd());
            crate::clipboard::listen(bar.win.hwnd());
            lang::register_shell_hook(bar.win.hwnd());
            foreground::watch(bar.win.hwnd(), MSG_FOREGROUND);
            status::watch(bar.win.hwnd(), MSG_STATUS);
            cover::watch(bar.win.hwnd(), MSG_COVER);
            if let Some(t) = &tray {
                t.set_notify(bar.win.hwnd(), MSG_TRAY);
                bar.reset_tray(Some(t));
            }
            if dock_allowed {
                dock = opened("dock", DockWin::open(bar.cfg.dock, MSG_DOCK_APPBAR));
                if let Some(d) = dock.as_mut() {
                    refresh_dock(d, &bar.cfg.dock_pins, &mut dock_order);
                    d.resnap_soon();
                    sync_dock_overlap(d);
                }
            }
            bar.present(true)?;
        } else if recapture {
            bar.refresh_backdrop()?;
            if let Some(d) = dock.as_mut() {
                let _ = d.recapture();
            }
            bar.present(true)?;
        }
        let new_minute = bar.refresh_clock();
        if new_minute {
            bar.refresh_cover(true); // страховка: окно могло закрыться, пока событие не пришло
            bar.refresh_island(); // Island могли запустить или перенастроить
            if dock.is_some() {
                dock_dirty_at.get_or_insert(Instant::now());
                if let Some(h) = &hidden_taskbar {
                    h.enforce(); // Проводник мог снова показать свою панель
                }
            }
        }
        if let Some(t) = &tray {
            if new_minute || tray_changed {
                t.raise(); // Проводник мог снова поднять своё окно над нашим
            }
            if tray_changed {
                bar.refresh_tray(t);
                // Окно открыто: трогаем его, только если изменился набор значков. Подсказки меняются у приложений
                // постоянно (скорость загрузки), пересоздавать из-за них окно нельзя: клик под курсором потеряется.
                if let Some(f) = flyout.as_mut() {
                    let now = t.icons();
                    if icon_signature(&now) != f.sig {
                        if flyout_size(f.tile_count(&bar, t, &now)) == flyout_size(f.icons.len() + f.fallbacks.len()) {
                            f.set_icons(&bar, t, now);
                        } else {
                            flyout = Some(FlyoutWin::open(&bar, t, f.center_x, f.show_hidden)?);
                        }
                    }
                }
            }
            if let Some(i) = bar.tray_click.take() {
                let icon = bar.pinned_icons.borrow().get(i).cloned();
                if let Some(icon) = icon {
                    t.send_click(&icon, Button::Left);
                }
            }
            if let Some(i) = bar.tray_right.take() {
                let icon = bar.pinned_icons.borrow().get(i).cloned();
                if let Some(icon) = icon {
                    t.send_click(&icon, Button::Right);
                }
            }
            if let Some(cx) = bar.tray_chevron.take() {
                (menu, control) = (None, None);
                if flyout.is_some() {
                    flyout = None;
                } else {
                    flyout = Some(FlyoutWin::open(&bar, t, cx, false)?);
                }
            }
            // Убранные значки показаны или спрятаны заново: окно собирается другого размера.
            let mut reopen: Option<(f32, bool)> = None;
            if let Some(f) = flyout.as_mut() {
                let mut close = f.handle_input();
                let actions: Vec<FlyoutAction> = f.actions.borrow_mut().drain(..).collect();
                let mut pins_changed = false;
                for action in actions {
                    match action {
                        FlyoutAction::Click(i) => {
                            if let Some(icon) = f.icons.get(i) {
                                t.send_click(icon, Button::Left);
                            } else if let Some(app) = f.fallbacks.get(i - f.icons.len()) {
                                activate_app(&app.exe_path);
                            }
                            close = true;
                        }
                        // Родное меню есть только у живых значков.
                        FlyoutAction::Right(i) => {
                            if let Some(icon) = f.icons.get(i) {
                                t.send_click(icon, Button::Right);
                                close = true;
                            }
                        }
                        FlyoutAction::Pin(i) => {
                            if let Some(exe) = f.icons.get(i).and_then(|icon| t.exe_of(icon)) {
                                bar.cfg.toggle_tray_pin(&exe);
                                let _ = bar.cfg.save();
                                pins_changed = true;
                            }
                        }
                        FlyoutAction::Hide(i) => {
                            let exe = match f.icons.get(i) {
                                Some(icon) => t.exe_of(icon),
                                None => i.checked_sub(f.icons.len()).and_then(|k| f.fallbacks.get(k)).map(|a| a.exe_path.clone()),
                            };
                            if let Some(exe) = exe {
                                bar.cfg.toggle_tray_hidden(&exe);
                                let _ = bar.cfg.save();
                                bar.refresh_tray(t); // убранный значок уходит и с панели
                                reopen = Some((f.center_x, f.show_hidden));
                            }
                        }
                        FlyoutAction::Reveal => reopen = Some((f.center_x, !f.show_hidden)),
                    }
                }
                if pins_changed {
                    bar.refresh_tray(t);
                    let all = f.all.clone();
                    f.set_icons(&bar, t, all);
                }
                f.present()?;
                if close {
                    flyout = None;
                }
            }
            if let Some((cx, show)) = reopen {
                flyout = Some(FlyoutWin::open(&bar, t, cx, show)?);
            }
        }
        bar.present(false)?;

        if let Some(i) = bar.pin_click.take() {
            if let Some(path) = bar.cfg.pins.get(i) {
                launch(path);
            }
        }
        if bar.menu_request.take().is_some() {
            (flyout, control) = (None, None);
            if menu.is_some() {
                menu = None;
            } else {
                let entries = menu::logo_menu(bar.cfg.logo.is_some(), bar.cfg.tray_mono, power::front_app().is_some());
                menu = Some(MenuWin::open(&bar, entries, None, MenuKind::Logo)?);
            }
        }
        if let Some((i, cx)) = bar.pin_menu_request.take() {
            menu = Some(MenuWin::open(&bar, menu::pin_menu(), Some(cx), MenuKind::Pin(i))?);
        }
        if bar.lang_request.take() {
            lang::switch_next();
        }
        match bar.quick_request.replace(QuickItem::None) {
            QuickItem::None => {}
            QuickItem::Spotlight => toggle_spotlight = true,
        }
        // Звук, Bluetooth и часы: одно всплывающее окно за раз. Ошибка открытия не роняет панель.
        if let Some(cx) = bar.volume_request.take() {
            (menu, flyout, calendar, control) = (None, None, None, None);
            volume = opened("volume", VolumeWin::open(bar.anchor(), cx));
        }
        if let Some(cx) = bar.clock_request.take() {
            (menu, flyout, volume, control) = (None, None, None, None);
            calendar = opened("notification center", CalendarWin::open_panel(bar.anchor(), cx, &LocalTime::now()));
        }
        if let Some(cx) = bar.app_name_request.take() {
            (flyout, volume, calendar, control) = (None, None, None, None);
            app_menu_target = bar.app_target.get();
            app_menu_more_anchor = cx;
            let entries = menu::app_menu(&bar.shown_name.borrow(), bar.app_target.get() != 0);
            menu = opened("app menu", MenuWin::open(&bar, entries, Some(cx), MenuKind::App));
        }
        if let Some((index, cx)) = bar.app_menu_request.take() {
            (flyout, volume, calendar, control) = (None, None, None, None);
            let target = bar.app_target.get();
            if bar.app_menu_more.get() == Some(index as usize) {
                let titles = appmenu::titles(target);
                app_menu_target = target;
                app_menu_more_anchor = cx;
                app_menu_more_first = index as usize;
                app_menu_more_offset = app_menu_more_first;
                menu = opened("app menu overflow", MenuWin::open(&bar, menu::app_more_menu(&titles, app_menu_more_first, app_menu_more_offset), Some(cx), MenuKind::App));
                continue;
            }
            let items = appmenu::items(target, index as usize, MAX_APP_ITEMS);
            if !items.is_empty() {
                app_menu_target = target;
                app_cmds = items.iter().map(|i| i.command).collect();
                menu = opened("app items", MenuWin::open(&bar, menu::app_items_menu(&items), Some(cx), MenuKind::App));
            }
        }
        if let Some(cx) = bar.clock_menu_request.take() {
            (flyout, volume, calendar, control) = (None, None, None, None);
            menu = opened("clock menu", MenuWin::open(&bar, menu::clock_menu(&bar.cfg.clock), Some(cx), MenuKind::Clock));
        }
        if let Some(cx) = bar.bluetooth_request.take() {
            if control.is_some() {
                control = None;
            } else {
                (menu, flyout, volume, calendar) = (None, None, None, None);
                control = opened("bluetooth", ControlCenterWin::open_list(bar.anchor(), cx, bar.view.ui.get_wifi(), CcPage::Bluetooth));
            }
        }
        if let Some(cx) = bar.battery_request.take() {
            (flyout, volume, calendar, control) = (None, None, None, None);
            // Замер нагрузки процессора занимает долю секунды: меню открывается сразу с готовым списком.
            let entries = menu::battery_menu(&battery::read(), &battery::top_cpu(battery::MAX_HOGS), bar.cfg.battery_percent);
            menu = opened("battery menu", MenuWin::open(&bar, entries, Some(cx), MenuKind::Battery));
        }
        if let Some((kind, dx, unit)) = bar.item_drag_request.take() {
            let steps = baritems::drag_steps(dx, unit);
            if steps != 0 {
                let mut order = baritems::parse_order(&bar.cfg.bar_order);
                let shown = bar.shown_kinds();
                for _ in 0..steps.abs() {
                    order = baritems::move_visible(&order, &shown, kind as usize, steps.signum());
                }
                bar.cfg.bar_order = if baritems::is_default_order(&order) { String::new() } else { baritems::serialize(&order) };
                let _ = bar.cfg.save();
                bar.refresh_items();
            }
        }
        if let Some((kind, cx)) = bar.item_menu_request.take() {
            (flyout, volume, calendar, control) = (None, None, None, None);
            let kind = kind as usize;
            let order = baritems::parse_order(&bar.cfg.bar_order);
            let shown = bar.shown_kinds();
            let can_left = baritems::move_visible(&order, &shown, kind, -1) != order;
            let can_right = baritems::move_visible(&order, &shown, kind, 1) != order;
            item_target = kind;
            menu = opened("item menu", MenuWin::open(&bar, menu::item_menu(kind, can_left, can_right), Some(cx), MenuKind::Item));
        }
        if let Some(cx) = bar.bar_menu_request.take() {
            (flyout, volume, calendar, control) = (None, None, None, None);
            let hidden = baritems::parse_hidden(&bar.cfg.bar_hidden);
            let entries = menu::bar_menu(&hidden, &bar.active_kinds(), bar.cfg.bar_autohide, bar.cfg.bar_solid);
            menu = opened("bar menu", MenuWin::open(&bar, entries, Some(cx), MenuKind::Item));
        }
        if let Some(cx) = bar.focus_request.take() {
            (flyout, volume, calendar, control) = (None, None, None, None);
            menu = opened("focus menu", MenuWin::open(&bar, menu::focus_menu(), Some(cx), MenuKind::Focus));
        }
        if let Some(cx) = bar.privacy_request.take() {
            (flyout, volume, calendar, control) = (None, None, None, None);
            let entries = menu::privacy_menu(&privacy::users(privacy::Device::Microphone), &privacy::users(privacy::Device::Camera));
            menu = opened("privacy menu", MenuWin::open(&bar, entries, Some(cx), MenuKind::Privacy));
        }
        if bar.vpn_request.take() {
            launch("ms-settings:network-vpn");
        }
        if let Some(cx) = bar.wifi_request.take() {
            if control.is_some() {
                control = None;
            } else {
                (menu, flyout, volume, calendar) = (None, None, None, None);
                control = opened("wifi", ControlCenterWin::open_list(bar.anchor(), cx, bar.view.ui.get_wifi(), CcPage::Wifi));
            }
        }
        if let Some(cx) = bar.control_request.take() {
            if control.is_some() {
                control = None;
            } else {
                (menu, flyout, volume, calendar) = (None, None, None, None);
                control = opened("control center", ControlCenterWin::open(bar.anchor(), cx, bar.view.ui.get_wifi()));
            }
        }
        if let Some(c) = &control {
            match c.tick() {
                Ok((close, open)) => {
                    if let Some(page) = open {
                        launch(page);
                    }
                    if close {
                        control = None;
                    }
                }
                Err(_) => control = None,
            }
        }
        if let Some(v) = &volume {
            if v.tick().unwrap_or(true) {
                volume = None;
            }
        }
        if let Some(c) = &calendar {
            if c.tick().unwrap_or(true) {
                calendar = None;
            }
        }
        if let Some(m) = &menu {
            let close = m.handle_input();
            m.present()?;
            let (picked, kind) = (m.picked.get(), m.kind);
            if picked >= 0 {
                // Меню закрывается до диалогов: оно держит захват мыши.
                menu = None;
                match picked {
                    ID_RESTART => {
                        // Новая копия подождёт, пока эта выйдет и отпустит замок.
                        let _ = std::process::Command::new(std::env::current_exe()?).arg("--restart").spawn();
                        return Ok(());
                    }
                    ID_QUIT => return Ok(()),
                    ID_PICK_LOGO => {
                        let picked_file = pick_file(
                            "Выбор логотипа",
                            "Картинки и значки",
                            "*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.ico;*.exe",
                        );
                        if let Some(path) = picked_file.filter(|p| load_icon(p, 16).is_some()) {
                            bar.cfg.logo = Some(path);
                            let _ = bar.cfg.save();
                            bar.apply_config();
                        }
                    }
                    ID_RESET_LOGO => {
                        bar.cfg.logo = None;
                        let _ = bar.cfg.save();
                        bar.apply_config();
                    }
                    ID_ADD_APP => {
                        if let Some(path) = pick_file("Приложение на панель", "Приложения", "*.exe;*.lnk") {
                            if !bar.cfg.pins.contains(&path) {
                                bar.cfg.pins.push(path);
                                let _ = bar.cfg.save();
                                bar.apply_config();
                            }
                        }
                    }
                    ID_TRAY_STYLE => {
                        bar.cfg.tray_mono = !bar.cfg.tray_mono;
                        bar.view.ui.set_tray_mono(bar.cfg.tray_mono);
                        let _ = bar.cfg.save();
                        bar.reset_tray(tray.as_ref());
                    }
                    ID_REMOVE_PIN => {
                        if let MenuKind::Pin(i) = kind {
                            bar.cfg.remove_pin(i);
                            let _ = bar.cfg.save();
                            bar.apply_config();
                        }
                    }
                    ID_DOCK_PIN | ID_DOCK_CLOSE | ID_DOCK_SMART | ID_DOCK_AUTOHIDE | ID_DOCK_FIXED => {
                        match picked {
                            ID_DOCK_PIN => {
                                if let Some(item) = &dock_menu_item {
                                    bar.cfg.toggle_dock_pin(&item.target);
                                    let _ = bar.cfg.save();
                                }
                            }
                            ID_DOCK_CLOSE => {
                                if let Some(item) = &dock_menu_item {
                                    close_item(item);
                                }
                            }
                            _ => {
                                bar.cfg.dock = match picked {
                                    ID_DOCK_AUTOHIDE => DockMode::Autohide,
                                    ID_DOCK_FIXED => DockMode::Fixed,
                                    _ => DockMode::Smart,
                                };
                                let _ = bar.cfg.save();
                                drop(dock.take()); // старый Dock снимает занятое место, новый занимает своё
                                dock = opened("dock", DockWin::open(bar.cfg.dock, MSG_DOCK_APPBAR));
                            }
                        }
                        if let Some(d) = dock.as_mut() {
                            refresh_dock(d, &bar.cfg.dock_pins, &mut dock_order);
                            sync_dock_overlap(d);
                        }
                    }
                    ID_APPS => {
                        (flyout, volume, calendar, spotlight, control) = (None, None, None, None, None);
                        if index_built.elapsed() > INDEX_MAX_AGE {
                            index_rx = spawn_index(bar.win.hwnd());
                            index_built = Instant::now();
                        }
                        apps = opened("apps", AppsWin::open(index.clone(), &usage));
                    }
                    id if (menu::ID_WIDGET_ITEM..menu::ID_WIDGET_ITEM + widgets::KINDS.len() as i32).contains(&id) => {
                        let kind = (id - menu::ID_WIDGET_ITEM) as usize;
                        if let Some(i) = desk_widgets.iter().position(|w| w.kind == kind) {
                            desk_widgets.remove(i);
                            widgets::remove(&mut bar.cfg.widgets, kind);
                        } else {
                            let (x, y) = widgets::default_position(desk_widgets.len(), kind, bar.win.rect().w as f32 / bar.scale);
                            if let Some(w) = opened("widget", WidgetWin::open(widgets::Spec::saved(&bar.cfg.widgets, kind, x, y), &bar.cfg.weather)) {
                                desk_widgets.push(w);
                                widgets::upsert(&mut bar.cfg.widgets, kind, x, y);
                            }
                        }
                        let _ = bar.cfg.save();
                    }
                    id if (menu::ID_WIDGET_SIZE..menu::ID_WIDGET_SIZE + widgets::SIZES.len() as i32).contains(&id) => {
                        widgets::set_size(&mut bar.cfg.widgets, widget_target, (id - menu::ID_WIDGET_SIZE) as usize);
                        let _ = bar.cfg.save();
                        reopen_widget(&mut desk_widgets, widget_target, &bar.cfg);
                    }
                    id if (menu::ID_WIDGET_COLOR..menu::ID_WIDGET_COLOR + widgets::COLORS.len() as i32).contains(&id) => {
                        widgets::set_color(&mut bar.cfg.widgets, widget_target, (id - menu::ID_WIDGET_COLOR) as usize);
                        let _ = bar.cfg.save();
                        reopen_widget(&mut desk_widgets, widget_target, &bar.cfg);
                    }
                    menu::ID_WIDGET_REMOVE => {
                        desk_widgets.retain(|w| w.kind != widget_target);
                        widgets::remove(&mut bar.cfg.widgets, widget_target);
                        let _ = bar.cfg.save();
                    }
                    menu::ID_MISSION => mission_request = true,
                    menu::ID_MACSHELL_SETTINGS => {
                        if settings_win.is_none() {
                            settings_win = opened("settings", SettingsWin::open(&bar.cfg));
                        }
                    }
                    menu::ID_WIDGET_RESET => {
                        let kinds: Vec<usize> = desk_widgets.iter().map(|w| w.kind).collect();
                        desk_widgets.clear();
                        bar.cfg.widgets.clear();
                        for (n, kind) in kinds.into_iter().enumerate() {
                            let (x, y) = widgets::default_position(n, kind, bar.win.rect().w as f32 / bar.scale);
                            if let Some(w) = opened("widget", WidgetWin::open(widgets::Spec::saved(&bar.cfg.widgets, kind, x, y), &bar.cfg.weather)) {
                                desk_widgets.push(w);
                                widgets::upsert(&mut bar.cfg.widgets, kind, x, y);
                            }
                        }
                        let _ = bar.cfg.save();
                    }
                    ID_ITEM_LEFT | ID_ITEM_RIGHT => {
                        let order = baritems::parse_order(&bar.cfg.bar_order);
                        let moved = baritems::move_visible(&order, &bar.shown_kinds(), item_target, if picked == ID_ITEM_LEFT { -1 } else { 1 });
                        bar.cfg.bar_order = if baritems::is_default_order(&moved) { String::new() } else { baritems::serialize(&moved) };
                        let _ = bar.cfg.save();
                        bar.refresh_items();
                    }
                    ID_ITEM_HIDE => {
                        let hidden = baritems::toggle_hidden(&baritems::parse_hidden(&bar.cfg.bar_hidden), item_target);
                        bar.cfg.bar_hidden = baritems::serialize(&hidden);
                        let _ = bar.cfg.save();
                        bar.refresh_items();
                    }
                    id if (ID_BAR_ITEM..ID_BAR_ITEM + baritems::KINDS.len() as i32).contains(&id) => {
                        let hidden = baritems::toggle_hidden(&baritems::parse_hidden(&bar.cfg.bar_hidden), (id - ID_BAR_ITEM) as usize);
                        bar.cfg.bar_hidden = baritems::serialize(&hidden);
                        let _ = bar.cfg.save();
                        bar.refresh_items();
                    }
                    ID_BAR_RESET => {
                        bar.cfg.bar_order.clear();
                        bar.cfg.bar_hidden.clear();
                        let _ = bar.cfg.save();
                        bar.refresh_items();
                    }
                    ID_BAR_SOLID => {
                        bar.cfg.bar_solid = !bar.cfg.bar_solid;
                        let _ = bar.cfg.save();
                        bar.refresh_cover(true);
                    }
                    ID_BAR_AUTOHIDE => {
                        bar.cfg.bar_autohide = !bar.cfg.bar_autohide;
                        let _ = bar.cfg.save();
                        // Место под панель решается при её создании: проще и надёжнее перезапуститься.
                        let _ = std::process::Command::new(std::env::current_exe()?).arg("--restart").spawn();
                        return Ok(());
                    }
                    menu::ID_FOCUS_OFF => focus::set(false),
                    menu::ID_FOCUS_SETTINGS => launch("ms-settings:notifications"),
                    ID_APP_HIDE | ID_APP_HIDE_OTHERS | ID_APP_QUIT => {
                        let target = bar.app_target.get();
                        if target != 0 {
                            let pid = appmenu::pid_of(target);
                            match picked {
                                ID_APP_HIDE => appmenu::hide_app(pid),
                                ID_APP_HIDE_OTHERS => appmenu::hide_others(pid),
                                _ => appmenu::quit_app(pid),
                            }
                        }
                    }
                    menu::ID_WINDOW_LAYOUT_MENU => {
                        menu = opened("window layout", MenuWin::open(&bar, menu::window_layout_menu(), Some(app_menu_more_anchor), MenuKind::App));
                    }
                    id if (menu::ID_WINDOW_LAYOUT_FIRST..menu::ID_WINDOW_LAYOUT_FIRST + 9).contains(&id) => {
                        use crate::window_layout::Layout::*;
                        let layouts = [Fill, Left, Right, Top, Bottom, TopLeft, TopRight, BottomLeft, BottomRight];
                        let _ = crate::window_layout::place(app_menu_target, layouts[(id - menu::ID_WINDOW_LAYOUT_FIRST) as usize]);
                    }
                    menu::ID_APPMENU_MORE_PREV | menu::ID_APPMENU_MORE_NEXT => {
                        let titles = appmenu::titles(app_menu_target);
                        app_menu_more_offset = if picked == menu::ID_APPMENU_MORE_PREV { app_menu_more_offset.saturating_sub(menu::APP_MORE_PAGE).max(app_menu_more_first) } else { app_menu_more_offset.saturating_add(menu::APP_MORE_PAGE) };
                        menu = opened("app menu overflow", MenuWin::open(&bar, menu::app_more_menu(&titles, app_menu_more_first, app_menu_more_offset), Some(app_menu_more_anchor), MenuKind::App));
                    }
                    id if (menu::ID_APPMENU_MORE..menu::ID_APPMENU_MORE + appmenu::MAX_TITLES as i32).contains(&id) => {
                        let index = (id - menu::ID_APPMENU_MORE) as usize;
                        let items = appmenu::items(app_menu_target, index, MAX_APP_ITEMS);
                        if !items.is_empty() {
                            app_cmds = items.iter().map(|i| i.command).collect();
                            menu = opened("app items", MenuWin::open(&bar, menu::app_items_menu(&items), Some(app_menu_more_anchor), MenuKind::App));
                        }
                    }
                    id if id >= ID_APPMENU_ITEM => {
                        if let Some(cmd) = app_cmds.get((id - ID_APPMENU_ITEM) as usize) {
                            appmenu::invoke(app_menu_target, *cmd);
                        }
                    }
                    ID_CLOCK_SECONDS | ID_CLOCK_12H | ID_CLOCK_DATE | ID_CLOCK_WEEKDAY => {
                        let c = &mut bar.cfg.clock;
                        match picked {
                            ID_CLOCK_SECONDS => c.seconds = !c.seconds,
                            ID_CLOCK_12H => c.hour12 = !c.hour12,
                            ID_CLOCK_DATE => c.hide_date = !c.hide_date,
                            _ => c.hide_weekday = !c.hide_weekday,
                        }
                        let _ = bar.cfg.save();
                        bar.refresh_clock();
                    }
                    ID_PRIVACY_MIC => launch("ms-settings:privacy-microphone"),
                    ID_PRIVACY_CAMERA => launch("ms-settings:privacy-webcam"),
                    ID_BATTERY_SAVER => launch("ms-settings:batterysaver"),
                    ID_BATTERY_SETTINGS => launch("ms-settings:powersleep"),
                    ID_BATTERY_PERCENT => {
                        bar.cfg.battery_percent = !bar.cfg.battery_percent;
                        let _ = bar.cfg.save();
                        bar.view.ui.set_battery_percent(bar.cfg.battery_percent);
                    }
                    menu::ID_WIDGETS => {
                        let mut present = [false; widgets::KINDS.len()];
                        for w in &desk_widgets {
                            present[w.kind] = true;
                        }
                        menu = opened("widgets menu", MenuWin::open(&bar, menu::widgets_menu(&present), Some(bar.win.rect().w as f32 / bar.scale / 2.0), MenuKind::Item));
                    }
                    ID_QUICK_KEYS => {
                        // Свои быстрые клавиши пишутся в конфиге: файл открывается в Блокноте (с подсказкой внутри).
                        if let Some(path) = Config::path() {
                            bar.cfg.add_quick_key_help();
                            let _ = bar.cfg.save();
                            let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
                        }
                    }
                    ID_ABOUT => launch("ms-settings:about"),
                    ID_SYSTEM_SETTINGS => launch("ms-settings:"),
                    ID_STORE => launch("ms-windows-store:"),
                    ID_RECENT => launch("shell:Recent"),
                    ID_FORCE_QUIT => {
                        power::force_quit_front();
                    }
                    ID_SLEEP => power::run(PowerAction::Sleep),
                    ID_RESTART_PC => power::run(PowerAction::Restart),
                    ID_SHUTDOWN => power::run(PowerAction::Shutdown),
                    ID_LOCK => power::run(PowerAction::Lock),
                    ID_LOGOFF => power::run(PowerAction::LogOff),
                    _ => {}
                }
            } else if close {
                menu = None;
            }
        }

        if let Some(c) = corners.as_mut() {
            for action in c.tick() {
                match action.as_str() {
                    "apps" => toggle_apps = true,
                    "spotlight" => toggle_spotlight = true,
                    "desktop" => {
                        desktop::toggle();
                    }
                    "lock" => power::run(PowerAction::Lock),
                    "mission" => mission_request = true,
                    "notifications" => panel_request = true,
                    "stage" => stage_request = true,
                    _ => {}
                }
            }
        }
        for cmd in switch_cmds {
            switch_command(cmd, &mut switch_session, &index);
        }
        if let Some(s) = switch_session.as_mut() {
            if s.win.is_none() && Instant::now() >= s.show_at {
                s.win = opened("switcher", SwitcherWin::open(&s.entries, s.selected));
                if s.win.is_none() {
                    s.show_at = Instant::now() + Duration::from_secs(3600); // не открылось: без окна переключение всё равно работает
                }
            }
        }
        let mut picked = None;
        if let Some(w) = switch_session.as_mut().and_then(|s| s.win.as_mut().map(|w| (w, &mut s.selected))) {
            match w.0.mouse() {
                Some(Mouse::Hover(i)) => {
                    *w.1 = i;
                    w.0.select(i);
                }
                Some(Mouse::Pick(i)) => picked = Some(i),
                None => {}
            }
            let _ = w.0.present(false);
        }
        if let Some(i) = picked {
            if let Some(s) = switch_session.take() {
                activate_entry(&s.entries, i);
            }
        }
        // Второе нажатие почти сразу после первого это нетерпение, а не просьба закрыть: сетка уже открывается.
        if toggle_apps && last_tap.is_none_or(|t| t.elapsed() > Duration::from_millis(400)) {
            last_tap = Some(Instant::now());
            // Одиночный Win: вместо Пуска Dock и сетка приложений; повторное нажатие закрывает сетку.
            (menu, flyout, volume, calendar, spotlight, control) = (None, None, None, None, None, None);
            if apps.is_some() {
                apps = None;
            } else {
                if index_built.elapsed() > INDEX_MAX_AGE {
                    index_rx = spawn_index(bar.win.hwnd());
                    index_built = Instant::now();
                }
                apps = opened("apps", AppsWin::open(index.clone(), &usage));
                if let Some(d) = dock.as_mut() {
                    d.show();
                }
            }
        }
        if toggle_spotlight {
            if spotlight.is_some() {
                spotlight = None;
                crate::host::trim_working_set();
            } else {
                (menu, flyout, volume, calendar, apps, control) = (None, None, None, None, None, None);
                if index_built.elapsed() > INDEX_MAX_AGE {
                    index_rx = spawn_index(bar.win.hwnd());
                    index_built = Instant::now();
                }
                let opened = Instant::now();
                // Быстрые клавиши читаются из файла при каждом открытии: правки подхватываются сразу.
                let quick = Arc::new(crate::quick::resolve(&Config::load().quick_keys));
                match SpotlightWin::open(index.clone(), &usage, files.clone(), quick) {
                    Ok(s) => {
                        trace(&format!("spotlight opened in {:?}", opened.elapsed()));
                        spotlight = Some(s);
                    }
                    Err(e) => trace(&format!("spotlight failed to open: {e:#}")),
                }
            }
        }
        let mut spotlight_animating = false;
        if let Some(s) = spotlight.as_mut() {
            match s.handle_input() {
                Outcome::Stay => {
                    // Вкладка «Файлы» просит индекс: строим при первой просьбе или когда он устарел.
                    if s.take_wants_files() || (files_rx.is_none() && files_built.elapsed() > FILES_MAX_AGE && files.is_some()) {
                        if files_rx.is_none() {
                            files_rx = Some(spawn_files(bar.win.hwnd()));
                        }
                    }
                    s.present()?;
                    spotlight_animating = s.tick()?;
                }
                Outcome::Close => {
                    spotlight = None;
                    crate::host::trim_working_set();
                }
                Outcome::Launch(what) => {
                    s.launched();
                    spotlight = None;
                    run_launch(&what, &mut usage);
                    crate::host::trim_working_set();
                }
                // Ctrl+1 или кнопка «Приложения»: вместо вкладки открывается сетка приложений.
                Outcome::OpenApps => {
                    spotlight = None;
                    if index_built.elapsed() > INDEX_MAX_AGE {
                        index_rx = spawn_index(bar.win.hwnd());
                        index_built = Instant::now();
                    }
                    apps = opened("apps", AppsWin::open(index.clone(), &usage));
                }
            }
        }

        let mut apps_animating = false;
        if let Some(a) = apps.as_mut() {
            match a.handle_input() {
                Outcome::Stay => {
                    if let Some(target) = a.take_removed() {
                        // Крестик на плитке: приложение уходит из «Приложений» и Spotlight насовсем (до правки конфига).
                        bar.cfg.hide_app(&target);
                        let _ = bar.cfg.save();
                        index = Arc::new(index.iter().filter(|e| e.target != target).cloned().collect());
                        a.set_index(index.clone());
                    }
                    a.present()?;
                    apps_animating = a.tick()?;
                    a.present()?;
                }
                Outcome::Close => {
                    apps = None;
                    crate::host::trim_working_set();
                }
                Outcome::Launch(what) => {
                    a.launched();
                    apps = None;
                    run_launch(&what, &mut usage);
                    crate::host::trim_working_set();
                }
                Outcome::OpenApps => {}
            }
        }

        // Dock: список окон после событий оболочки, клики по значкам, скольжение.
        let mut dock_animating = false;
        if dock.is_some() {
            if dock_dirty_at.is_some_and(|at| Instant::now() >= at) {
                dock_dirty_at = None;
                if let Some(d) = dock.as_mut() {
                    refresh_dock(d, &bar.cfg.dock_pins, &mut dock_order);
                    sync_dock_overlap(d); // новое окно или другая ширина подложки
                }
            }
            let events = dock.as_mut().map(|d| d.handle_input()).unwrap_or_default();
            for event in events {
                let Some(d) = dock.as_ref() else { break };
                match event {
                    DockEvent::Launcher => {
                        (menu, flyout, volume, calendar, spotlight, control) = (None, None, None, None, None, None);
                        if apps.is_some() {
                            apps = None;
                        } else {
                            if index_built.elapsed() > INDEX_MAX_AGE {
                                index_rx = spawn_index(bar.win.hwnd());
                                index_built = Instant::now();
                            }
                            apps = opened("apps", AppsWin::open(index.clone(), &usage));
                        }
                    }
                    DockEvent::Click(n) => {
                        if let Some(item) = d.items().get(n) {
                            if item.running() {
                                activate_item(item);
                            } else {
                                launch(&item.target);
                            }
                        }
                    }
                    DockEvent::Downloads => launch("shell:Downloads"),
                    DockEvent::Trash => launch("shell:RecycleBinFolder"),
                    DockEvent::Menu(n, cx) => {
                        if let Some(item) = d.items().get(n).cloned() {
                            let entries = menu::dock_menu(&item.name, item.pinned, item.running(), bar.cfg.dock);
                            let top = d.top_px();
                            (flyout, volume, calendar, control) = (None, None, None, None);
                            menu = opened("dock menu", MenuWin::open_placed(&bar, entries, Some(cx), MenuKind::Dock, Some(top)));
                            dock_menu_item = menu.as_ref().map(|_| item);
                        }
                    }
                }
            }
            if let Some(d) = dock.as_mut() {
                d.set_hold((dock_menu_item.is_some() && menu.is_some()) || apps.is_some());
                d.present()?;
                dock_animating = d.tick()?;
                d.present()?;
            }
        }

        // Кадры анимаций: окна, название, подсветка под курсором. В покое таймеров нет.
        if !other_bars.is_empty() {
            let (name, clock) = (bar.shown_name.borrow().clone(), bar.shown_clock.borrow().clone());
            for b in other_bars.iter_mut() {
                b.sync(&name, &clock);
                let _ = b.handle_input();
                let _ = b.present(false);
            }
        }
        let neighbors: Vec<_> = desk_widgets.iter().map(|w| {
            let (x, y) = w.position_pt();
            widgets::Spec { x, y, ..w.spec() }
        }).collect();
        for w in desk_widgets.iter_mut() {
            if let Ok(Some((x, y))) = w.tick(&neighbors) {
                widgets::upsert(&mut bar.cfg.widgets, w.kind, x, y);
                let _ = bar.cfg.save();
            }
        }
        if let Some((kind, (cx, cy))) = desk_widgets.iter().find_map(|w| w.take_context().map(|c| (w.kind, c))) {
            (flyout, volume, calendar, control) = (None, None, None, None);
            let spec = widgets::entries(&bar.cfg.widgets).into_iter().find(|s| s.kind == kind).unwrap_or(widgets::Spec::new(kind, 0, 0));
            let entries = menu::widget_menu(&spec);
            let s = bar.scale;
            let h = (menu::height(&entries) * s).ceil() as i32;
            widget_target = kind;
            menu = opened("widget menu", MenuWin::open_placed(&bar, entries, Some(cx as f32 / s + menu::WIDTH / 2.0), MenuKind::Item, Some(cy + h + (6.0 * s).round() as i32)));
        }
        if crate::requests::take(crate::requests::Request::Stage) || stage_request {
            bar.cfg.stage_on = !bar.cfg.stage_on;
            let _ = bar.cfg.save();
            if bar.cfg.stage_on {
                stage = Stage::open().ok();
                if let Some(s) = stage.as_mut() {
                    let front = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize;
                    let _ = s.update(&crate::winlist::list(), front);
                }
            } else if let Some(mut s) = stage.take() {
                s.restore_all();
            }
        }
        if let Some(s) = stage.as_mut() {
            if let Ok(Some(h)) = s.tick() {
                crate::host::bring_to_front(h);
            }
        }
        if crate::requests::take(crate::requests::Request::Settings) && settings_win.is_none() {
            settings_win = opened("settings", SettingsWin::open(&bar.cfg));
        }
        if crate::requests::take(crate::requests::Request::Calendar) {
            if let Some(widget) = desk_widgets.iter().find(|w| w.kind == 1) {
                widget.focus_calendar();
            } else {
                let cx = bar.win.rect().w as f32 / bar.scale - 160.0;
                calendar = opened("calendar", CalendarWin::open(bar.anchor(), cx, &LocalTime::now()));
            }
        }
        if panel_request || crate::requests::take(crate::requests::Request::Panel) {
            (menu, flyout, volume, control) = (None, None, None, None);
            let cx = bar.win.rect().w as f32 / bar.scale - 160.0;
            calendar = if calendar.is_some() { None } else { opened("notification center", CalendarWin::open_panel(bar.anchor(), cx, &LocalTime::now())) };
        }
        if mission_request || crate::requests::take(crate::requests::Request::Mission) {
            (menu, flyout, volume, calendar, spotlight, control) = (None, None, None, None, None, None);
            mission_win = if mission_win.is_some() { None } else { opened("mission control", MissionWin::open(&crate::winlist::list())) };
        }
        let mission_outcome = mission_win.as_ref().map(|m| (m.tick(), m.desktops()));
        match mission_outcome {
            Some((Ok(mission::Outcome::Stay), _)) | None => {}
            Some((Ok(outcome), (count, current))) => {
                mission_win = None;
                match outcome {
                    mission::Outcome::Activate(h) => crate::host::bring_to_front(h),
                    mission::Outcome::Desktop(i) => switch_desktop(current, i.min(count.saturating_sub(1))),
                    mission::Outcome::NewDesktop => winkey::send_chord(&[(VK_LWIN_KEY, false), (VK_CTRL_KEY, false), (b'D' as u16, false)]),
                    _ => {}
                }
            }
            Some((Err(_), _)) => mission_win = None,
        }
        if let Some(w) = &settings_win {
            match w.tick(&mut bar.cfg) {
                Ok((true, dirty)) => {
                    settings_win = None;
                    if dirty {
                        // Цвета, значки и клавиши читаются при создании окон: перезапуск надёжнее, чем перестройка на лету.
                        let _ = std::process::Command::new(std::env::current_exe()?).arg("--restart").spawn();
                        return Ok(());
                    }
                }
                Ok(_) => {}
                Err(_) => settings_win = None,
            }
        }
        let hold = menu.is_some() || flyout.is_some() || volume.is_some() || calendar.is_some() || control.is_some() || spotlight.is_some() || apps.is_some();
        let ah_anim = bar.tick_autohide(hold);
        let (name_anim, bar_anim) = (bar.tick_name(), bar.view.animating());
        let mix_anim = bar.tick_mix();
        let window_motion = crate::host::tick_window_animations();
        let mut animating = window_motion || name_anim || bar_anim || ah_anim || other_bars.iter().any(|b| b.animating()) || desk_widgets.iter().any(|w| w.animating()) || mix_anim || spotlight_animating || apps_animating || dock_animating;
        if let Some(m) = menu.as_mut() {
            animating |= m.view.animating();
        }
        if let Some(f) = flyout.as_mut() {
            animating |= f.view.animating();
        }
        animating |= volume.as_ref().is_some_and(|v| v.animating()) || calendar.as_ref().is_some_and(|c| c.animating());
        animating |= control.as_ref().is_some_and(|c| c.animating());
        animating |= stage.as_ref().is_some_and(|s| s.animating());
        animating |= mission_win.as_ref().is_some_and(|w| w.animating());
        animating |= settings_win.as_ref().is_some_and(|w| w.animating());
        animating |= switch_session.as_ref().and_then(|s| s.win.as_ref()).is_some_and(|w| w.animating());
        if animating {
            trace(&format!("animating: name={name_anim} bar={bar_anim} menu={} flyout={}", menu.is_some(), flyout.is_some()));
        }

        // Один раз после старта: панель заняла своё место, обои под ней читаются без окон.
        if let Some(at) = bar.recheck_backdrop_at.get() {
            let now = Instant::now();
            if now >= at {
                bar.recheck_backdrop_at.set(None);
                bar.refresh_backdrop()?;
            }
        }
        let mut wait = ms_until_next_tick(&LocalTime::now(), &bar.cfg.clock) + 20;
        wait = wait.min(bar.ah_wake_ms());
        for w in &desk_widgets {
            wait = wait.min(w.next_wake_ms());
        }
        if let Some(at) = bar.recheck_backdrop_at.get() {
            wait = wait.min(at.saturating_duration_since(Instant::now()).as_millis() as u32 + 5);
        }
        if animating {
            wait = wait.min(FRAME_MS);
        }
        if menu.is_some() || flyout.is_some() || volume.is_some() || calendar.is_some() || control.is_some() {
            wait = wait.min(50); // пока окно открыто, ловим Esc опросом
        }
        if let Some(s) = &spotlight {
            wait = wait.min(s.next_wake_ms()); // мигание курсора
        }
        if let Some(c) = &corners {
            wait = wait.min(c.next_wake_ms());
        }
        if let Some(s) = switch_session.as_ref().filter(|s| s.win.is_none()) {
            wait = wait.min(s.show_at.saturating_duration_since(Instant::now()).as_millis() as u32 + 1); // пора показать ряд значков
        }
        if let Some(a) = &apps {
            wait = wait.min(a.next_wake_ms());
        }
        if let Some(d) = &dock {
            wait = wait.min(d.next_wake_ms());
            if let Some(at) = dock_dirty_at {
                wait = wait.min(at.saturating_duration_since(Instant::now()).as_millis() as u32 + 1);
            }
        }
        if !pump_messages(wait) {
            return Ok(());
        }
    }
}

/// Открытый переключатель приложений: список, выбор и окно (оно появляется, если Alt держат дольше `SHOW_AFTER`).
struct SwitchSession {
    entries: Vec<switcher::Entry>,
    selected: usize,
    show_at: Instant,
    win: Option<SwitcherWin>,
}

fn activate_entry(entries: &[switcher::Entry], i: usize) {
    if let Some(h) = entries.get(i).and_then(|e| e.hwnds.first().copied()) {
        crate::host::bring_to_front(h);
    }
}

/// Команда от хука клавиатуры (`winkey::Switch`, номер в `cmd`).
fn switch_command(cmd: usize, session: &mut Option<SwitchSession>, known: &[Entry]) {
    use crate::winkey::Switch;
    let is = |s: Switch| cmd == s as usize;
    if is(Switch::Next) || is(Switch::Prev) {
        let delta = if is(Switch::Next) { 1 } else { -1 };
        match session {
            Some(s) => {
                s.selected = switcher::step(s.selected, s.entries.len(), delta);
                if let Some(w) = s.win.as_mut() {
                    w.select(s.selected);
                }
            }
            None => {
                let entries = switcher::entries(known);
                if !entries.is_empty() {
                    let selected = switcher::first_index(entries.len(), delta < 0);
                    *session = Some(SwitchSession { entries, selected, show_at: Instant::now() + switcher::SHOW_AFTER, win: None });
                }
            }
        }
    } else if is(Switch::Commit) {
        if let Some(s) = session.take() {
            activate_entry(&s.entries, s.selected);
        }
    } else if is(Switch::Cancel) {
        *session = None;
    } else if is(Switch::Quit) || is(Switch::Hide) {
        let Some(s) = session.as_mut() else { return };
        if let Some(h) = s.entries.get(s.selected).and_then(|e| e.hwnds.first().copied()) {
            let pid = appmenu::pid_of(h);
            if is(Switch::Quit) {
                appmenu::quit_app(pid);
            } else {
                appmenu::hide_app(pid);
            }
            s.entries.remove(s.selected);
        }
        if s.entries.is_empty() {
            *session = None;
        } else {
            s.selected = s.selected.min(s.entries.len() - 1);
            s.win = None; // ряд стал короче: окно откроется заново по новому списку
            s.show_at = Instant::now();
        }
    } else if is(Switch::SameApp) {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let front = unsafe { GetForegroundWindow() }.0 as isize;
        if let Some(e) = switcher::entries(known).into_iter().find(|e| e.hwnds.contains(&front)) {
            if e.hwnds.len() > 1 {
                if let Some(h) = pick_window(&e.hwnds, front) {
                    crate::host::bring_to_front(h);
                }
            }
        }
    }
}

const VK_LWIN_KEY: u16 = 0x5B;
const VK_CTRL_KEY: u16 = 0x11;

/// Переходит с рабочего стола `from` на стол `to` сочетаниями Ctrl+Win+стрелка.
fn switch_desktop(from: usize, to: usize) {
    let arrow: u16 = if to > from { 0x27 } else { 0x25 };
    for _ in 0..from.abs_diff(to) {
        winkey::send_chord(&[(VK_LWIN_KEY, false), (VK_CTRL_KEY, false), (arrow, true)]);
        std::thread::sleep(Duration::from_millis(60));
    }
}

/// Какое окно приложения вынести вперёд по клику на значок: `None` значит «свернуть». Окна идут сверху вниз.
/// Впереди окно приложения: одно сворачивается, из нескольких берётся следующее; иначе верхнее окно.
pub fn pick_window(hwnds: &[isize], foreground: isize) -> Option<isize> {
    match hwnds.iter().position(|&h| h == foreground) {
        Some(_) if hwnds.len() == 1 => None,
        Some(i) => Some(hwnds[(i + 1) % hwnds.len()]),
        None => hwnds.first().copied(),
    }
}

/// Клик по значку запущенного приложения.
fn activate_item(item: &DockItem) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SW_MINIMIZE, ShowWindow};
    let front = unsafe { GetForegroundWindow() }.0 as isize;
    match pick_window(&item.hwnds, front) {
        Some(h) => crate::host::bring_to_front(h),
        None => unsafe {
            let _ = ShowWindow(HWND(front as *mut _), SW_MINIMIZE);
        },
    }
}

/// «Закрыть окна»: мягкая просьба, как крестик.
fn close_item(item: &DockItem) {
    use windows::Win32::Foundation::HWND;
    for &h in &item.hwnds {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Начальный набор Dock: Проводник. Кладётся один раз, чтобы пустой Dock не выглядел сломанным.
fn seed_dock(cfg: &mut Config) {
    if cfg.dock_seeded {
        return;
    }
    cfg.dock_seeded = true;
    if cfg.dock_pins.is_empty() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        cfg.dock_pins.push(format!(r"{root}\explorer.exe"));
    }
    let _ = cfg.save();
}

/// Читает окна и обновляет значки Dock, если набор изменился.
fn refresh_dock(dock: &mut DockWin, pins: &[String], order: &mut Vec<String>) {
    let items = dock::merge(pins, &crate::winlist::list(), order);
    if items != dock.items() {
        dock.set_items(items);
    }
}

/// Умный Dock: прячется под нижний край, пока окно приложения задевает его подложку.
fn sync_dock_overlap(dock: &mut DockWin) {
    if dock.mode() == DockMode::Smart {
        let band = dock.pill_rect();
        dock.set_overlap(dock_overlapped(&cover::snapshot(), band));
    }
}

/// Результат открытия окна: ошибка уходит в журнал, панель продолжает работать.
fn opened<T>(what: &str, result: Result<T>) -> Option<T> {
    result.map_err(|e| trace(&format!("{what} failed to open: {e:#}"))).ok()
}

/// Отметки времени запуска для `MACSHELL_TRACE=1` (куда уходит время до готовности полосы).
fn trace(label: &str) {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    if std::env::var_os("MACSHELL_TRACE").is_some() {
        eprintln!("[{:>6.1} ms] {label}", start.elapsed().as_secs_f64() * 1000.0);
    }
}

/// Окно значков трея с настоящими значками нескольких программ, для проверки вида глазами (`--snapshot-tray=`).
pub fn snapshot_tray(path: &str, mono: bool) -> Result<()> {
    set_dpi_awareness()?;
    let scale = primary_scale();
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let apps = ["notepad", "calc", "mspaint", "taskmgr", "cmd", "winver", "regedit", "charmap", "mstsc", "msinfo32"];
    let px = (24.0 * scale).round() as i32;
    let items: Vec<TrayItem> = apps
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let icon = load_icon(&format!(r"{sys}\System32\{name}.exe"), px)
                .map(|p| if mono { to_mono(&p) } else { p })
                .map(|p| slint_image(&p))
                .unwrap_or_default();
            TrayItem { limited: i == 7, icon, name: name.to_string().into(), tip: format!("{name}.exe").into(), pinned: i == 1 || i == 4, hidden: i == 9 }
        })
        .collect();
    let (lw, lh) = flyout_size(items.len());
    let (w, h) = ((lw * scale).round() as i32, (lh * scale).ceil() as i32);
    let rect = RectI { x: 900, y: (28.0 * scale) as i32, w, h };
    let view = ContentView::new_with(w as u32, h as u32, TrayFlyout::new);
    view.set_scale_factor(scale);
    view.ui.set_items(ModelRc::new(VecModel::from(items)));
    view.ui.set_hover_name("notepad".into());
    view.ui.set_hover_tip("notepad.exe".into());
    let mut shot = capture_region(rect)?;
    let glass = popup_glass(rect, &flyout_glass().scaled(scale))?;
    let mut out = glass;
    cpu::composite_over(&mut out, &view.render());
    cpu::composite_over(&mut shot, &out.pixels);
    write_bmp(path, &shot)
}


/// По чему узнаём, что набор значков изменился: окно, номер и дескриптор значка (подсказки не в счёт).
fn icon_signature(icons: &[TrayIcon]) -> Vec<(isize, u32, isize)> {
    icons.iter().map(|i| (i.hwnd, i.uid, i.hicon)).collect()
}

#[cfg(test)]
mod dock_click_tests {
    use super::pick_window;

    #[test]
    fn app_not_in_front_comes_to_the_front_with_its_top_window() {
        assert_eq!(pick_window(&[7, 8], 1), Some(7));
    }

    #[test]
    fn single_front_window_is_minimised() {
        assert_eq!(pick_window(&[7], 7), None);
    }

    #[test]
    fn several_windows_cycle_from_the_front_one_and_wrap() {
        assert_eq!(pick_window(&[7, 8, 9], 7), Some(8));
        assert_eq!(pick_window(&[7, 8, 9], 9), Some(7));
    }
}

#[cfg(test)]
mod signature_tests {
    use super::*;

    #[test]
    fn long_app_menus_reserve_space_for_overflow() {
        let titles = vec!["Файл".to_string(), "Правка".to_string(), "Вид".to_string()];
        assert_eq!(visible_menu_count(&titles, 100.0, 500.0), 3);
        assert_eq!(visible_menu_count(&titles, 100.0, 225.0), 1);
        assert_eq!(visible_menu_count(&titles, 100.0, 155.0), 0);
    }

    fn icon(tip: &str, hicon: isize) -> TrayIcon {
        TrayIcon { hwnd: 5, uid: 1, guid: None, callback_msg: 0x800, hicon, tip: tip.into(), hidden: false, version: 0 }
    }

    #[test]
    fn display_and_dpi_changes_rebuild_the_bar() {
        assert!(rebuilds_on(WM_DISPLAYCHANGE));
        assert!(rebuilds_on(WM_DPICHANGED));
        assert!(!rebuilds_on(WM_SETTINGCHANGE));
    }

    #[test]
    fn signature_ignores_tooltips_but_not_icon_changes() {
        assert_eq!(icon_signature(&[icon("Загрузка: 1 Б/с", 7)]), icon_signature(&[icon("Загрузка: 9 Б/с", 7)]));
        assert_ne!(icon_signature(&[icon("x", 7)]), icon_signature(&[icon("x", 8)]));
    }
}
