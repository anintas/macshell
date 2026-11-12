//! Окно с попиксельной прозрачностью (`UpdateLayeredWindow`) и цикл сообщений.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Once;
use std::time::Instant;

use anyhow::{Result, bail};
use capture::{RectI, exclude_from_capture};
use glass::Bitmap;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY,
    VK_CONTROL, VK_SHIFT,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetThreadDpiAwarenessContext,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

static REGISTER: Once = Once::new();

/// События окна для главного цикла. Координаты в физических пикселях окна.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    Move { x: i32, y: i32 },
    Down { x: i32, y: i32 },
    Up { x: i32, y: i32 },
    RightUp { x: i32, y: i32 },
    Leave,
    /// Набранный символ (`WM_CHAR`, пары суррогатов уже склеены). Управляющие символы не приходят.
    Char(char),
    /// Нажатая клавиша управления или сочетание с Ctrl: стрелки, Enter, Esc, Backspace, Tab, Ctrl+V и т.п.
    Key { vk: u32, ctrl: bool, shift: bool },
    /// Колесо мыши: 120 на один щелчок, положительное значение вперёд (от себя).
    Wheel { delta: i32 },
    /// Системные сообщения, которые окно само не разбирает: `WM_APP + n`, зарегистрированные
    /// (например `TaskbarCreated`), смена экрана, настроек, питания и времени.
    Message { msg: u32, wparam: usize, lparam: isize },
}

const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const WM_MOUSELEAVE: u32 = 0x02A3;
const MA_NOACTIVATE: isize = 3;

thread_local! {
    /// Очередь событий всех окон потока: (окно, событие).
    static QUEUE: RefCell<Vec<(isize, InputEvent)>> = const { RefCell::new(Vec::new()) };
}

fn push(hwnd: HWND, ev: InputEvent) {
    QUEUE.with(|q| q.borrow_mut().push((hwnd.0 as isize, ev)));
}

fn xy(lp: LPARAM) -> (i32, i32) {
    (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32)
}

/// Склеивает пары суррогатов из `WM_CHAR` (эмодзи приходят двумя сообщениями). Одиночный суррогат без пары
/// и управляющие символы (меньше пробела, DEL) дают `None`.
pub fn join_utf16_char(pending: &mut Option<u16>, unit: u16) -> Option<char> {
    if (0xD800..0xDC00).contains(&unit) {
        *pending = Some(unit);
        return None;
    }
    if (0xDC00..0xE000).contains(&unit) {
        let high = pending.take()?;
        return char::decode_utf16([high, unit]).next()?.ok();
    }
    *pending = None;
    char::from_u32(unit as u32).filter(|c| !c.is_control())
}

thread_local! {
    static PENDING_HIGH: Cell<Option<u16>> = const { Cell::new(None) };
}

/// Клавиши, которые окно передаёт как `InputEvent::Key` (остальное приходит символами).
fn is_control_key(vk: u32, ctrl: bool) -> bool {
    matches!(
        vk,
        0x08 | 0x09 | 0x0D | 0x1B | 0x21..=0x28 | 0x2E // Backspace, Tab, Enter, Esc, PgUp..Down, Delete
    ) || (ctrl && ((0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk)))
}

unsafe fn track_leave(hwnd: HWND) {
    let mut t = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        let _ = TrackMouseEvent(&mut t);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            unsafe { track_leave(hwnd) };
            push(hwnd, InputEvent::Move { x, y });
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let (x, y) = xy(lp);
            push(hwnd, InputEvent::Down { x, y });
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            push(hwnd, InputEvent::Wheel { delta: (wp.0 >> 16) as u16 as i16 as i32 });
            return LRESULT(0);
        }
        WM_RBUTTONUP => {
            let (x, y) = xy(lp);
            push(hwnd, InputEvent::RightUp { x, y });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let (x, y) = xy(lp);
            push(hwnd, InputEvent::Up { x, y });
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            push(hwnd, InputEvent::Leave);
            return LRESULT(0);
        }
        // У окон нет значка курсора в классе: без этого над панелью остаётся «загрузка».
        WM_SETCURSOR => {
            unsafe {
                if let Ok(arrow) = LoadCursorW(None, IDC_ARROW) {
                    SetCursor(Some(arrow));
                }
            }
            return LRESULT(1);
        }
        WM_CLOSE => {
            push(hwnd, InputEvent::Message { msg, wparam: wp.0, lparam: lp.0 });
            return LRESULT(0);
        }
        WM_CHAR => {
            let unit = wp.0 as u16;
            if let Some(c) = PENDING_HIGH.with(|p| {
                let mut pending = p.get();
                let c = join_utf16_char(&mut pending, unit);
                p.set(pending);
                c
            }) {
                push(hwnd, InputEvent::Char(c));
            }
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u32;
            let down = |key: VIRTUAL_KEY| unsafe { GetKeyState(key.0 as i32) } < 0;
            let ctrl = down(VK_CONTROL);
            if is_control_key(vk, ctrl) {
                push(hwnd, InputEvent::Key { vk, ctrl, shift: down(VK_SHIFT) });
            }
            return LRESULT(0);
        }
        // Окно потеряло активность (клик в другое окно, Alt+Tab): всплывающему окну пора закрыться.
        WM_ACTIVATE if wp.0 & 0xFFFF == 0 => {
            push(hwnd, InputEvent::Message { msg, wparam: wp.0, lparam: lp.0 });
        }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE),
        WM_POWERBROADCAST => {
            push(hwnd, InputEvent::Message { msg, wparam: wp.0, lparam: lp.0 });
            return LRESULT(1);
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_TIMECHANGE | WM_DWMCOLORIZATIONCOLORCHANGED | WM_DPICHANGED | WM_CLIPBOARDUPDATE => {
            push(hwnd, InputEvent::Message { msg, wparam: wp.0, lparam: lp.0 });
        }
        m if m >= WM_APP => push(hwnd, InputEvent::Message { msg, wparam: wp.0, lparam: lp.0 }),
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Делает окно `target` активным (наше или чужое) и отдаёт ему клавиатуру. Свёрнутое окно разворачивается.
pub fn bring_to_front(target: isize) {
    let hwnd = HWND(target as *mut _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        // Пустая клавиша от нашего процесса: после неё система не считает передний план «чужим».
        crate::winkey::send_noop_key();
        if SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd {
            let _ = SetFocus(Some(hwnd));
            return;
        }
        let front = GetForegroundWindow();
        let front_thread = GetWindowThreadProcessId(front, None);
        let me = GetCurrentThreadId();
        let attached = front_thread != 0 && front_thread != me && AttachThreadInput(me, front_thread, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if attached {
            let _ = AttachThreadInput(me, front_thread, false);
        }
        if GetForegroundWindow() != hwnd {
            SwitchToThisWindow(hwnd, true); // как Alt+Tab: не подчиняется блокировке переднего плана
        }
    }
}

/// Память под картинку окна: DIB 32 бита сверху вниз, живёт вместе с окном.
struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    len: usize,
}

/// Окно, уже отпущенное владельцем: больше не принимает ввод и мягко исчезает.
struct RetiringWindow {
    hwnd: HWND,
    rect: RectI,
    surface: Surface,
    tween: crate::anim::WindowTween,
    started: Instant,
    duration: u32,
}

impl Drop for RetiringWindow {
    fn drop(&mut self) {
        capture::forget_window(self.hwnd.0 as isize);
        unsafe { let _ = DestroyWindow(self.hwnd); }
        let _ = QUEUE.try_with(|q| q.borrow_mut().retain(|(hwnd, _)| *hwnd != self.hwnd.0 as isize));
    }
}

struct EnteringWindow {
    hwnd: HWND,
    dc: HDC,
    rect: Rc<Cell<RectI>>,
    alpha: Rc<Cell<u8>>,
    dy: Rc<Cell<i32>>,
    tween: crate::anim::WindowTween,
    started: Cell<Option<Instant>>,
}

thread_local! {
    static ENTERING: RefCell<Vec<Weak<EnteringWindow>>> = const { RefCell::new(Vec::new()) };
    static RETIRING: RefCell<Vec<RetiringWindow>> = const { RefCell::new(Vec::new()) };
}

fn commit_surface(hwnd: HWND, rect: RectI, dc: HDC, alpha: u8, dy: i32) -> Result<()> {
    unsafe {
        let dst = POINT { x: rect.x, y: rect.y + dy };
        let size = SIZE { cx: rect.w, cy: rect.h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(hwnd, None, Some(&dst), Some(&size), Some(dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA)?;
    }
    Ok(())
}

/// Продвигает только активные переходы; возвращает true, пока нужен следующий кадр.
pub fn tick_window_animations() -> bool {
    let mut active = false;
    ENTERING.with(|list| list.borrow_mut().retain(|weak| {
        let Some(window) = weak.upgrade() else { return false };
        let Some(started) = window.started.get() else { return true };
        let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        let (alpha, dy) = window.tween.sample(elapsed);
        window.alpha.set((alpha * 255.0).round().clamp(0.0, 255.0) as u8);
        window.dy.set(dy.round() as i32);
        let _ = commit_surface(window.hwnd, window.rect.get(), window.dc, window.alpha.get(), window.dy.get());
        let running = !window.tween.finished(elapsed);
        active |= running;
        running
    }));
    RETIRING.with(|list| list.borrow_mut().retain(|window| {
        let elapsed = window.started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        if elapsed >= window.duration { return false; }
        let (alpha, dy) = window.tween.sample(elapsed);
        let _ = commit_surface(window.hwnd, window.rect, window.surface.dc, (alpha * 255.0).round().clamp(0.0, 255.0) as u8, dy.round() as i32);
        active = true;
        true
    }));
    active
}

#[cfg(test)]
mod motion_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn native_overlay_releases_input_and_finishes_its_close() {
        if !crate::anim::enabled() { return; }
        let foreground = unsafe { GetForegroundWindow() };
        let mut window = LayeredWindow::new(RectI { x: -200, y: -200, w: 32, h: 32 }).unwrap();
        let hwnd = window.hwnd;
        window.animate_overlay(true, -8);
        let mut pixels = Bitmap::new(32, 32);
        pixels.pixels.fill(0xff808080);
        window.present(&pixels).unwrap();
        assert_eq!(window.blend(), (0, -8));
        std::thread::sleep(Duration::from_millis(65));
        assert!(tick_window_animations());
        assert!((1..255).contains(&window.blend().0));
        drop(window);
        assert!(unsafe { IsWindow(Some(hwnd)).as_bool() });
        assert_ne!(unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 } & WS_EX_TRANSPARENT.0, 0);
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        std::thread::sleep(Duration::from_millis(150));
        tick_window_animations();
        assert!(!unsafe { IsWindow(Some(hwnd)).as_bool() });
    }
}

impl Surface {
    fn new(w: i32, h: i32) -> Result<Surface> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                bail!("CreateCompatibleDC failed");
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(dc);
                    bail!("CreateDIBSection failed");
                }
            };
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            Ok(Surface { dc, bmp, old, bits: bits as *mut u32, len: (w as usize) * (h as usize) })
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

pub struct LayeredWindow {
    hwnd: HWND,
    rect: Rc<Cell<RectI>>,
    surface: Option<Surface>,
    /// Общая прозрачность и сдвиг по вертикали (анимации появления); 255 и 0 в покое.
    alpha: Rc<Cell<u8>>,
    dy: Rc<Cell<i32>>,
    entering: Option<Rc<EnteringWindow>>,
    exit_dy: Cell<Option<i32>>,
    presented: Cell<bool>,
}

impl LayeredWindow {
    /// Окно без рамки поверх всех, не забирает фокус и невидимо для захвата экрана.
    pub fn new(rect: RectI) -> Result<LayeredWindow> {
        LayeredWindow::create(rect, false)
    }

    /// То же окно, но может быть активным и получать клавиатуру (Spotlight). Фокус берётся вызовом `focus`.
    pub fn new_focusable(rect: RectI) -> Result<LayeredWindow> {
        LayeredWindow::create(rect, true)
    }

    fn create(rect: RectI, focusable: bool) -> Result<LayeredWindow> {
        if rect.w <= 0 || rect.h <= 0 {
            bail!("window size must be positive, got {}x{}", rect.w, rect.h);
        }
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            REGISTER.call_once(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: hinst.into(),
                    lpszClassName: w!("macshell_layered"),
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    ..Default::default()
                };
                RegisterClassW(&wc);
            });
            let no_activate = if focusable { WINDOW_EX_STYLE(0) } else { WS_EX_NOACTIVATE };
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | no_activate,
                w!("macshell_layered"),
                PCWSTR::null(),
                WS_POPUP,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                None,
                None,
                Some(hinst.into()),
                None,
            )?;
            let win = LayeredWindow {
                hwnd,
                rect: Rc::new(Cell::new(rect)),
                surface: Some(Surface::new(rect.w, rect.h)?),
                alpha: Rc::new(Cell::new(255)),
                dy: Rc::new(Cell::new(0)),
                entering: None,
                exit_dy: Cell::new(None),
                presented: Cell::new(false),
            };
            exclude_from_capture(hwnd.0 as isize)?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Ok(win)
        }
    }

    /// Переход для всплывающего окна. `enter=false` сохраняет собственное появление окна.
    pub fn animate_overlay(&mut self, enter: bool, distance_px: i32) {
        if !crate::anim::enabled() { return; }
        self.exit_dy.set(Some(distance_px / 2));
        if enter {
            self.alpha.set(0);
            self.dy.set(distance_px);
            let window = Rc::new(EnteringWindow {
                hwnd: self.hwnd,
                dc: self.surface.as_ref().expect("window surface").dc,
                rect: self.rect.clone(), alpha: self.alpha.clone(), dy: self.dy.clone(),
                tween: crate::anim::WindowTween::new((0.0, distance_px as f32), (1.0, 0.0), 180),
                started: Cell::new(None),
            });
            ENTERING.with(|list| list.borrow_mut().push(Rc::downgrade(&window)));
            self.entering = Some(window);
        }
    }

    /// Делает окно активным и отдаёт ему клавиатуру. Из фонового процесса `SetForegroundWindow` часто отказывает:
    /// тогда подключаемся к потоку окна, которое сейчас впереди, и повторяем.
    pub fn focus(&self) {
        bring_to_front(self.hwnd.0 as isize);
    }

    /// Поверх всех окон (`true`) или под всеми (`false`, для полноэкранных приложений).
    pub fn set_topmost(&self, on: bool) {
        let after = if on { HWND_TOPMOST } else { HWND_BOTTOM };
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Ставит окно в порядке окон сразу под `above`, не меняя положения и активности.
    pub fn place_below(&self, above: isize) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND(above as *mut _)), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Это окно сейчас активно (впереди и держит клавиатуру).
    pub fn is_foreground(&self) -> bool {
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    /// Захват мыши: клики вне окна приходят окну (с координатами за его границами). Для всплывающих меню.
    pub fn capture_mouse(&self, on: bool) {
        unsafe {
            if on {
                SetCapture(self.hwnd);
            } else {
                let _ = ReleaseCapture();
            }
        }
    }

    /// Заголовок окна: по нему `--quit` находит запущенную полосу.
    pub fn set_title(&self, title: &str) {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(wide.as_ptr()));
        }
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd.0 as isize
    }

    pub fn ex_style(&self) -> u32 {
        unsafe { GetWindowLongW(self.hwnd, GWL_EXSTYLE) as u32 }
    }

    pub fn rect(&self) -> RectI {
        self.rect.get()
    }

    /// Забирает накопленные события этого окна (мышь и системные сообщения).
    pub fn take_input(&self) -> Vec<InputEvent> {
        let me = self.hwnd.0 as isize;
        QUEUE.with(|q| {
            let mut mine = Vec::new();
            q.borrow_mut().retain(|(h, ev)| {
                if *h == me {
                    mine.push(*ev);
                    false
                } else {
                    true
                }
            });
            mine
        })
    }

    /// Двигает окно; картинка обновится при следующем `present`.
    pub fn set_position(&self, x: i32, y: i32) -> Result<()> {
        let r = self.rect.get();
        unsafe {
            SetWindowPos(self.hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)?;
        }
        self.rect.set(RectI { x, y, ..r });
        Ok(())
    }

    /// Показывает предумноженный BGRA размером с окно.
    pub fn present(&self, bmp: &Bitmap) -> Result<()> {
        let r = self.rect.get();
        let surface = self.surface.as_ref().expect("window surface");
        if bmp.width as i32 != r.w || bmp.height as i32 != r.h || bmp.pixels.len() != surface.len {
            bail!("bitmap {}x{} does not match window {}x{}", bmp.width, bmp.height, r.w, r.h);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bmp.pixels.as_ptr(), surface.bits, surface.len);
        }
        self.presented.set(true);
        if let Some(entering) = &self.entering {
            if entering.started.get().is_none() { entering.started.set(Some(Instant::now())); }
        }
        self.commit()
    }

    /// Общая прозрачность окна (0..255) и сдвиг по вертикали в пикселях: для появления и исчезновения.
    /// Картинка не пересчитывается, система просто заново накладывает уже готовый кадр.
    pub fn set_blend(&self, alpha: u8, dy: i32) -> Result<()> {
        self.alpha.set(alpha);
        self.dy.set(dy);
        self.commit()
    }

    pub fn blend(&self) -> (u8, i32) {
        (self.alpha.get(), self.dy.get())
    }

    /// Показывает текущее содержимое поверхности с заданными прозрачностью и сдвигом.
    fn commit(&self) -> Result<()> {
        commit_surface(self.hwnd, self.rect.get(), self.surface.as_ref().expect("window surface").dc, self.alpha.get(), self.dy.get())
    }
}

impl Drop for LayeredWindow {
    fn drop(&mut self) {
        unsafe {
            if GetCapture() == self.hwnd { let _ = ReleaseCapture(); }
        }
        if let Some(distance) = self.exit_dy.get().filter(|_| self.presented.get() && self.alpha.get() > 0 && !self.is_foreground() && crate::anim::enabled()) {
            unsafe {
                let style = GetWindowLongW(self.hwnd, GWL_EXSTYLE) as u32;
                let _ = SetWindowLongW(self.hwnd, GWL_EXSTYLE, (style | WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0) as i32);
                if GetWindowLongW(self.hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0 {
                    if let Some(surface) = self.surface.take() {
                        let from = (self.alpha.get() as f32 / 255.0, self.dy.get() as f32);
                        RETIRING.with(|list| list.borrow_mut().push(RetiringWindow {
                            hwnd: self.hwnd, rect: self.rect.get(), surface,
                            tween: crate::anim::WindowTween::new(from, (0.0, distance as f32), 120),
                            started: Instant::now(), duration: 120,
                        }));
                        return;
                    }
                }
            }
        }
        capture::forget_window(self.hwnd.0 as isize);
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        let _ = QUEUE.try_with(|q| q.borrow_mut().retain(|(hwnd, _)| *hwnd != self.hwnd.0 as isize));
    }
}

/// Ждёт сообщения до таймаута и обрабатывает их. `false`, когда получено `WM_QUIT`.
pub fn pump_messages(timeout_ms: u32) -> bool {
    unsafe {
        MsgWaitForMultipleObjects(None, false, timeout_ms, QS_ALLINPUT);
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

/// Отдаёт системе страницы, которые процесс сейчас не использует (библиотеки оболочки после сборки индекса,
/// картинки закрытого Spotlight). Нужные вернутся при первом обращении; в диспетчере задач память в покое честно мала.
pub fn trim_working_set() {
    unsafe {
        let _ = windows::Win32::System::ProcessStatus::EmptyWorkingSet(windows::Win32::System::Threading::GetCurrentProcess());
    }
}

/// Масштаб по DPI: 96 это 1.0, 120 это 1.25. Ноль (ошибка системы) считается за 1.0.
pub fn scale_from_dpi(dpi: u32) -> f32 {
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

/// Текущий масштаб основного монитора. `GetDpiForSystem` для процесса Per-Monitor V2 возвращает масштаб,
/// который был при входе в систему: если владелец сменит 100% на 125% без выхода, полоса осталась бы мелкой.
pub fn primary_scale() -> f32 {
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForSystem, MDT_EFFECTIVE_DPI};
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let (mut x, mut y) = (0u32, 0u32);
        match GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) {
            Ok(()) if x > 0 => scale_from_dpi(x),
            _ => scale_from_dpi(GetDpiForSystem()),
        }
    }
}

/// Per-Monitor V2. Повторный вызов в уже настроенном процессе тоже `Ok`.
pub fn set_dpi_awareness() -> Result<()> {
    unsafe {
        let set = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let current = GetThreadDpiAwarenessContext();
        if set.is_ok() || AreDpiAwarenessContextsEqual(current, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool() {
            Ok(())
        } else {
            bail!("cannot enable per-monitor v2 DPI awareness")
        }
    }
}

#[cfg(test)]
mod char_tests {
    use super::*;

    #[test]
    fn plain_characters_pass_and_controls_do_not() {
        let mut p = None;
        assert_eq!(join_utf16_char(&mut p, 'я' as u16), Some('я'));
        assert_eq!(join_utf16_char(&mut p, 'a' as u16), Some('a'));
        assert_eq!(join_utf16_char(&mut p, ' ' as u16), Some(' '));
        for control in [0x08u16, 0x09, 0x0D, 0x1B, 0x16, 0x7F] {
            assert_eq!(join_utf16_char(&mut p, control), None, "{control:#x}");
        }
    }

    #[test]
    fn surrogate_pairs_are_joined_and_lone_ones_dropped() {
        let mut p = None;
        let (high, low) = ('😀'.encode_utf16(&mut [0; 2])[0], '😀'.encode_utf16(&mut [0; 2])[1]);
        assert_eq!(join_utf16_char(&mut p, high), None);
        assert_eq!(join_utf16_char(&mut p, low), Some('😀'));
        assert_eq!(join_utf16_char(&mut p, low), None, "low surrogate without a high one");
        assert_eq!(join_utf16_char(&mut p, high), None);
        assert_eq!(join_utf16_char(&mut p, 'x' as u16), Some('x'), "a plain char cancels the pending high surrogate");
        assert_eq!(join_utf16_char(&mut p, low), None);
    }

    #[test]
    fn control_keys_are_forwarded_letters_only_with_ctrl() {
        for vk in [0x08, 0x09, 0x0D, 0x1B, 0x26, 0x28, 0x2E, 0x21, 0x22] {
            assert!(is_control_key(vk, false), "{vk:#x}");
        }
        assert!(!is_control_key(0x56, false)); // V без Ctrl приходит символом
        assert!(is_control_key(0x56, true));
        assert!(!is_control_key(0x20, false)); // пробел приходит символом
    }
}

#[cfg(test)]
mod dpi_tests {
    use super::*;

    #[test]
    fn scale_follows_dpi() {
        assert_eq!(scale_from_dpi(96), 1.0);
        assert_eq!(scale_from_dpi(120), 1.25);
        assert_eq!(scale_from_dpi(144), 1.5);
        assert_eq!(scale_from_dpi(0), 1.0);
    }

    #[test]
    fn primary_scale_is_plausible() {
        let s = primary_scale();
        assert!((1.0..=5.0).contains(&s), "{s}");
    }
}
