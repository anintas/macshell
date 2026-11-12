//! Edge tiling for a window dragged by its caption. No input is synthesized.
use std::cell::RefCell;
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use capture::RectI;
use glass::Bitmap;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, GA_ROOT, GetAncestor, GetCursorPos,
    GetMessageW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, GWL_EXSTYLE, GWL_STYLE, HTCAPTION, IsWindow, IsWindowVisible,
    KillTimer, MSG, PeekMessageW, PM_NOREMOVE, PostThreadMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, SetTimer,
    SetWindowLongW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WindowFromPoint,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WH_MOUSE_LL, WM_APP, WM_LBUTTONDOWN, WM_NCHITTEST, WM_QUIT, WM_TIMER,
    MSLLHOOKSTRUCT, WS_EX_TRANSPARENT, WS_THICKFRAME,
};
use crate::host::LayeredWindow;
use crate::window_layout::{self, Layout};

thread_local! { static EVENTS: RefCell<Vec<(u32, isize)>> = const { RefCell::new(Vec::new()) }; }
#[derive(Clone, Copy)]
struct MouseDown { hwnd: isize, x: i32, y: i32, time: Instant }
static DOWN: Mutex<Option<MouseDown>> = Mutex::new(None);

unsafe extern "system" fn on_mouse(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == 0 && wp.0 as u32 == WM_LBUTTONDOWN && lp.0 != 0 {
        let p = unsafe { (*(lp.0 as *const MSLLHOOKSTRUCT)).pt };
        unsafe {
            let hwnd = GetAncestor(WindowFromPoint(p), GA_ROOT);
            let mut r = RECT::default();
            if !hwnd.0.is_null() && GetWindowRect(hwnd, &mut r).is_ok() {
                *DOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(MouseDown { hwnd: hwnd.0 as isize, x: p.x-r.left, y: p.y-r.top, time: Instant::now() });
            } else { *DOWN.lock().unwrap_or_else(|e| e.into_inner()) = None; }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

unsafe extern "system" fn on_move(_: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, child: i32, _: u32, _: u32) {
    if object == 0 && child == 0 && !hwnd.0.is_null() {
        EVENTS.with(|q| q.borrow_mut().push((event, hwnd.0 as isize)));
        // Out-of-context callbacks may arrive while GetMessage is waiting; wake the loop
        // so that START is processed before a timer has been installed.
        let _ = unsafe { PostThreadMessageW(GetCurrentThreadId(), WM_APP + 71, WPARAM(0), LPARAM(0)) };
    }
}

/// The menu bar is a valid top target; bottom corners take precedence over sides.
pub fn zone(screen: RectI, work: RectI, x: i32, y: i32, threshold: i32) -> Option<Layout> {
    let t = threshold.max(1);
    if x < screen.x || y < screen.y || x >= screen.x + screen.w || y >= screen.y + screen.h { return None; }
    let left = x < screen.x + t;
    let right = x >= screen.x + screen.w - t;
    let top = y <= work.y + t;
    let bottom = y >= screen.y + screen.h - t;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(Layout::TopLeft),
        (_, true, true, _) => Some(Layout::TopRight),
        (true, _, _, true) => Some(Layout::BottomLeft),
        (_, true, _, true) => Some(Layout::BottomRight),
        (true, _, _, _) => Some(Layout::Left),
        (_, true, _, _) => Some(Layout::Right),
        (_, _, true, _) => Some(Layout::Fill),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Target { layout: Layout, area: RectI, dpi: u32 }

fn target_at(hwnd: HWND, p: POINT) -> Option<Target> {
    unsafe {
        let monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() { return None; }
        let rect = |r: windows::Win32::Foundation::RECT| RectI { x: r.left, y: r.top, w: r.right-r.left, h: r.bottom-r.top };
        let (mut dpi_x, mut dpi_y) = (0, 0);
        let dpi = if windows::Win32::UI::HiDpi::GetDpiForMonitor(monitor, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok() && dpi_x > 0 {
            dpi_x
        } else {
            windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96)
        };
        let screen = rect(info.rcMonitor);
        let area = rect(info.rcWork);
        zone(screen, area, p.x, p.y, (16 * dpi as i32 + 48) / 96).map(|layout| Target { layout, area, dpi })
    }
}

fn cursor() -> Option<POINT> { let mut p = POINT::default(); unsafe { GetCursorPos(&mut p).ok()?; } Some(p) }
fn trace(message: &str) { if std::env::var_os("MACSHELL_DRAG_TRACE").is_some() { eprintln!("{message}"); } }

fn is_caption_drag(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || !IsWindowVisible(hwnd).as_bool() || GetAncestor(hwnd, GA_ROOT) != hwnd
            || GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME.0 == 0 { return false; }
        let Some(down) = DOWN.lock().unwrap_or_else(|e| e.into_inner()).take() else { return false };
        if down.hwnd != hwnd.0 as isize || (GetAsyncKeyState(VK_LBUTTON.0 as i32) >= 0 && down.time.elapsed() > Duration::from_millis(500)) { return false; }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() { return false; }
        let p = POINT { x: rect.left + down.x, y: rect.top + down.y };
        let packed = ((p.x as u16 as u32) | ((p.y as u16 as u32) << 16)) as isize;
        let mut hit = 0usize;
        let result = SendMessageTimeoutW(hwnd, WM_NCHITTEST, WPARAM(0), LPARAM(packed), SMTO_ABORTIFHUNG, 100, Some(&mut hit));
        trace(&format!("start hit: success={} hit={}", result.0 != 0, hit));
        result.0 != 0 && is_caption_hit(hit)
    }
}

fn is_caption_hit(hit: usize) -> bool { hit == HTCAPTION as usize }

/// A click-through translucent outline; transparent corners keep the preview subtle.
fn preview(target: Target) -> Option<LayeredWindow> {
    let gap = (8 * target.dpi as i32 + 48) / 96;
    let r = window_layout::rectangle(target.area, target.layout, gap);
    let win = LayeredWindow::new(r).ok()?;
    unsafe {
        let hwnd = HWND(win.hwnd() as *mut _);
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        SetWindowLongW(hwnd, GWL_EXSTYLE, style | WS_EX_TRANSPARENT.0 as i32);
    }
    let bmp = preview_bitmap(r.w, r.h, target.dpi);
    win.present(&bmp).ok()?;
    Some(win)
}

fn preview_bitmap(width: i32, height: i32, dpi: u32) -> Bitmap {
    let mut bmp = Bitmap::new(width as u32, height as u32);
    let radius = (12 * dpi as i32 / 96).max(3);
    let border = (2 * dpi as i32 / 96).max(1);
    for y in 0..height {
        for x in 0..width {
            let dx = (radius - x).max(0).max(x - (width - radius - 1));
            let dy = (radius - y).max(0).max(y - (height - radius - 1));
            if dx * dx + dy * dy > radius * radius { continue; }
            let edge = x < border || y < border || x >= width - border || y >= height - border || dx * dx + dy * dy >= (radius-border).pow(2);
            bmp.pixels[(y * width + x) as usize] = if edge { 0x80727479 } else { 0x4036383c };
        }
    }
    bmp
}

/// Keep the low-level hook on a separate thread so painting a preview or asking
/// an unresponsive application for its hit test never stalls mouse input.
struct MouseWatcher { tid: u32, thread: Option<JoinHandle<()>> }
impl MouseWatcher {
    fn start() -> Option<Self> {
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().name("macshell-window-mouse".into()).spawn(move || unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
            let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(on_mouse), Some(hinst), 0) {
                Ok(h) => h,
                Err(_) => { let _ = tx.send(0); return; }
            };
            let _ = tx.send(GetCurrentThreadId());
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(hook);
        }).ok()?;
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(tid) if tid != 0 => Some(Self { tid, thread: Some(thread) }),
            _ => None,
        }
    }
}
impl Drop for MouseWatcher {
    fn drop(&mut self) {
        unsafe { let _ = PostThreadMessageW(self.tid, WM_QUIT, WPARAM(0), LPARAM(0)); }
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

pub struct WindowDrag { tid: u32, thread: Option<JoinHandle<()>>, _mouse: MouseWatcher }

impl WindowDrag {
    pub fn start() -> Option<Self> {
        let mouse = MouseWatcher::start()?;
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().name("macshell-window-drag".into()).spawn(move || unsafe {
            // Create the queue before exposing the thread id to Drop.
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            let hook = SetWinEventHook(EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND, None, Some(on_move), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
            if hook.0.is_null() { let _ = tx.send(0); return; }
            let _ = tx.send(GetCurrentThreadId());
            let mut active = 0isize;
            let mut candidate: Option<Target> = None;
            let mut overlay: Option<LayeredWindow> = None;
            let mut timer = 0usize;
            loop {
                let status = GetMessageW(&mut msg, None, 0, 0).0;
                if status <= 0 { break; }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                for (event, raw) in EVENTS.with(|q| q.borrow_mut().drain(..).collect::<Vec<_>>()) {
                    if event == EVENT_SYSTEM_MOVESIZESTART {
                        overlay = None;
                        candidate = None;
                        let hwnd = HWND(raw as *mut _);
                        active = if is_caption_drag(hwnd) { raw } else { 0 };
                        trace(&format!("move start: eligible={}", active != 0));
                        if active != 0 && timer == 0 { timer = SetTimer(None, 0, 30, None); }
                        if active == 0 && timer != 0 { let _ = KillTimer(None, timer); timer = 0; }
                    } else if event == EVENT_SYSTEM_MOVESIZEEND && raw == active {
                        let hwnd = HWND(raw as *mut _);
                        let valid = GetAsyncKeyState(VK_LBUTTON.0 as i32) >= 0;
                        let next = cursor().and_then(|p| target_at(hwnd, p));
                        trace(&format!("move end: eligible={valid} zone={:?}", next.map(|t| t.layout)));
                        overlay = None;
                        if valid { if let Some(t) = next { let _ = window_layout::place_in_area_with_dpi(raw, t.layout, t.area, t.dpi); } }
                        active = 0;
                        candidate = None;
                        if timer != 0 { let _ = KillTimer(None, timer); timer = 0; }
                    }
                }
                if msg.message == WM_TIMER && active != 0 {
                    let hwnd = HWND(active as *mut _);
                    let mut pid = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    if pid == 0 || !IsWindow(Some(hwnd)).as_bool() {
                        active = 0; overlay = None; candidate = None;
                        if timer != 0 { let _ = KillTimer(None, timer); timer = 0; }
                        continue;
                    }
                    let next = cursor().and_then(|p| target_at(hwnd, p));
                    if next != candidate {
                        trace(&format!("preview zone={:?}", next.map(|t| t.layout)));
                        overlay = None;
                        candidate = next;
                        if let Some(t) = next { overlay = preview(t); }
                    }
                }
            }
            drop(overlay);
            if timer != 0 { let _ = KillTimer(None, timer); }
            let _ = UnhookWinEvent(hook);
        }).ok()?;
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(tid) if tid != 0 => Some(Self { tid, thread: Some(thread), _mouse: mouse }),
            _ => None,
        }
    }
}

impl Drop for WindowDrag {
    fn drop(&mut self) {
        unsafe { let _ = PostThreadMessageW(self.tid, WM_QUIT, WPARAM(0), LPARAM(0)); }
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zones_respect_top_bar_edges_and_negative_monitors() {
        let screen = RectI { x: -1920, y: 0, w: 1920, h: 1080 };
        let work = RectI { x: -1920, y: 27, w: 1920, h: 1053 };
        assert_eq!(zone(screen, work, -1920, 400, 16), Some(Layout::Left));
        assert_eq!(zone(screen, work, -1, 400, 16), Some(Layout::Right));
        assert_eq!(zone(screen, work, -1000, 27, 16), Some(Layout::Fill));
        assert_eq!(zone(screen, work, -1920, 30, 16), Some(Layout::TopLeft));
        assert_eq!(zone(screen, work, -1, 1079, 16), Some(Layout::BottomRight));
        assert_eq!(zone(screen, work, -900, 500, 16), None);
        assert_eq!(zone(screen, work, 1, 500, 16), None);
    }

    #[test]
    fn caption_hit_does_not_accept_resize_borders() {
        use windows::Win32::UI::WindowsAndMessaging::{HTLEFT, HTRIGHT, HTTOP, HTBOTTOM, HTTOPLEFT, HTBOTTOMRIGHT};
        assert!(is_caption_hit(HTCAPTION as usize));
        for hit in [HTLEFT, HTRIGHT, HTTOP, HTBOTTOM, HTTOPLEFT, HTBOTTOMRIGHT] {
            assert!(!is_caption_hit(hit as usize));
        }
    }

    #[test]
    fn preview_has_transparent_corners_and_a_translucent_center() {
        let bmp = preview_bitmap(300, 200, 96);
        assert_eq!(bmp.pixels[0], 0);
        assert_eq!(bmp.pixels[100 * 300 + 150], 0x4036383c);
        if let Ok(path) = std::env::var("MACSHELL_DRAG_PREVIEW") {
            let mut background = Bitmap { width: 300, height: 200, pixels: vec![0xff30333a; 300 * 200] };
            glass::cpu::composite_over(&mut background, &bmp.pixels);
            crate::snapshot::write_bmp(&path, &background).unwrap();
        }
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn hooks_start_and_stop_without_leaving_threads() {
        let start = Instant::now();
        let drag = WindowDrag::start().expect("install mouse and move hooks");
        drop(drag);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
