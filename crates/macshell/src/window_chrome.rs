//! Experimental caption overlay for one explicitly selected standard Win32 window.
//! It is intentionally not installed globally: apps with custom title bars need adapters.
use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use capture::RectI;
use glass::Bitmap;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{ClientToScreen, GetDC, GetPixel, ReleaseDC};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetClassNameW, GetForegroundWindow, GetMessageW, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId, GWL_EXSTYLE, GWL_STYLE,
    IsIconic, IsWindow, IsWindowVisible, IsZoomed, KillTimer, MSG, PeekMessageW, PM_NOREMOVE, PostMessageW,
    PostThreadMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SetTimer, SetWindowLongW, ShowWindow, SW_HIDE, TranslateMessage,
    WM_QUIT, WM_SYSCOMMAND, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
    WS_SYSMENU,
};
use crate::host::LayeredWindow;
use crate::slint_view::{CaptionBackground, CaptionLights, ContentView};

#[derive(Clone, Debug, PartialEq)]
struct CaptionInfo { rect: RectI, dpi: u32, title: String }

/// Avoid custom client-drawn captions and windows without all three system actions.
fn caption_info(hwnd: HWND) -> Option<CaptionInfo> {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() { return None; }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let required = WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0;
        if style & required != required { return None; }
        let mut class = [0u16; 128];
        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n.min(class.len())]);
        if class == "CabinetWClass" || class.starts_with("Chrome_WidgetWin_") || class == "ApplicationFrameWindow" { return None; }
        let mut r = RECT::default();
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).ok()?;
        let mut client = POINT::default();
        if !ClientToScreen(hwnd, &mut client).as_bool() { return None; }
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
        let h = client.y - r.top;
        let min = (22 * dpi as i32 + 48) / 96;
        let max = (52 * dpi as i32 + 48) / 96;
        if h < min || h > max || r.right - r.left < (300 * dpi as i32 + 48) / 96 { return None; }
        let mut text = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut text).max(0) as usize;
        Some(CaptionInfo { rect: RectI { x: r.left, y: r.top, w: r.right-r.left, h }, dpi, title: String::from_utf16_lossy(&text[..n.min(text.len())]) })
    }
}

fn sample_color(r: RectI) -> [u8; 3] {
    unsafe {
        let dc = GetDC(None);
        if dc.0.is_null() { return [32, 32, 32]; }
        let rgb = GetPixel(dc, r.x + r.w * 2 / 3, r.y + r.h / 2).0;
        let _ = ReleaseDC(None, dc);
        if rgb == u32::MAX { [32, 32, 32] } else { [rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8] }
    }
}

struct Overlay {
    info: CaptionInfo,
    base: LayeredWindow,
    lights: LayeredWindow,
    mask: LayeredWindow,
    view: ContentView<CaptionLights>,
    picked: Rc<Cell<i32>>,
    base_image: Bitmap,
    shared: Option<crate::caption_shared::CaptionMapping>,
}

impl Overlay {
    fn open(target: HWND, info: CaptionInfo, color: [u8; 3]) -> Option<Self> {
        let r = info.rect;
        let scale = info.dpi as f32 / 96.0;
        let lights_w = (78.0 * scale).round() as i32;
        let mask_w = ((138.0 * scale).round() as i32).min(r.w - lights_w);
        if lights_w <= 0 || mask_w <= 0 { return None; }
        let ink = if (color[0] as u32 * 299 + color[1] as u32 * 587 + color[2] as u32 * 114) / 1000 > 140 { [30,30,30] } else { [240,240,240] };
        let base = LayeredWindow::new(r).ok()?;
        unsafe {
            let h = HWND(base.hwnd() as *mut _);
            let ex = GetWindowLongW(h, GWL_EXSTYLE);
            SetWindowLongW(h, GWL_EXSTYLE, ex | WS_EX_TRANSPARENT.0 as i32);
        }
        let bg = ContentView::new_with(r.w as u32, r.h as u32, CaptionBackground::new);
        bg.set_scale_factor(scale);
        bg.ui.set_caption_text(info.title.as_str().into());
        bg.ui.set_surface(slint::Color::from_rgb_u8(color[0], color[1], color[2]));
        bg.ui.set_ink(slint::Color::from_rgb_u8(ink[0], ink[1], ink[2]));
        let base_image = Bitmap { width:r.w as u32, height:r.h as u32, pixels:bg.render() };
        base.present(&base_image).ok()?;
        let mask = LayeredWindow::new(RectI { x:r.x+r.w-mask_w, y:r.y, w:mask_w, h:r.h }).ok()?;
        let argb = 0xff000000 | ((color[0] as u32)<<16) | ((color[1] as u32)<<8) | color[2] as u32;
        mask.present(&Bitmap { width:mask_w as u32, height:r.h as u32, pixels:vec![argb; (mask_w*r.h) as usize] }).ok()?;
        let lights = LayeredWindow::new(RectI { x:r.x, y:r.y, w:lights_w, h:r.h }).ok()?;
        let view = ContentView::new_with(lights_w as u32, r.h as u32, CaptionLights::new);
        view.set_scale_factor(scale);
        let picked = Rc::new(Cell::new(-1));
        let slot = picked.clone();
        view.ui.on_activated(move |action| slot.set(action));
        let light_pixels = view.render();
        lights.present(&Bitmap { width:lights_w as u32, height:r.h as u32, pixels:light_pixels.clone() }).ok()?;
        // Only the isolated test helper requests a discoverable/capturable light window.
        #[cfg(debug_assertions)]
        if std::env::var_os("MACSHELL_CAPTION_TEST_VISIBLE").is_some() {
            use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WINDOW_DISPLAY_AFFINITY};
            let h = HWND(lights.hwnd() as *mut _);
            unsafe {
                let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
                SetWindowLongW(h, GWL_EXSTYLE, ((ex & !WS_EX_TOOLWINDOW.0) | WS_EX_APPWINDOW.0) as i32);
                for raw in [base.hwnd(), mask.hwnd(), lights.hwnd()] {
                    let _ = SetWindowDisplayAffinity(HWND(raw as *mut _), WINDOW_DISPLAY_AFFINITY(0));
                }
            }
            lights.set_title("macshell.caption.test");
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(target, Some(&mut pid)); }
        let shared = crate::caption_shared::CaptionMapping::new(pid, target.0 as isize, r.w as usize);
        let overlay = Self { info, base, lights, mask, view, picked, base_image, shared };
        overlay.publish(&light_pixels);
        Some(overlay)
    }

    fn owns(&self, hwnd: HWND) -> bool {
        [self.base.hwnd(), self.lights.hwnd(), self.mask.hwnd()].contains(&(hwnd.0 as isize))
    }

    fn move_to(&mut self, info: CaptionInfo) -> bool {
        if self.info.rect.w != info.rect.w || self.info.rect.h != info.rect.h || self.info.dpi != info.dpi || self.info.title != info.title { return false; }
        let r = info.rect;
        if r != self.info.rect {
            let _ = self.base.set_position(r.x, r.y);
            let _ = self.mask.set_position(r.x + r.w - self.mask.rect().w, r.y);
            let _ = self.lights.set_position(r.x, r.y);
            self.info = info;
        }
        true
    }

    fn input(&self, target: HWND) {
        for ev in self.lights.take_input() { self.view.dispatch(&ev); }
        if let Some(pixels) = self.view.render_if_changed() {
            let r = self.lights.rect();
            self.publish(&pixels);
            let _ = self.lights.present(&Bitmap { width:r.w as u32, height:r.h as u32, pixels });
        }
        let command = match self.picked.replace(-1) {
            0 => Some(SC_CLOSE), 1 => Some(SC_MINIMIZE),
            2 => Some(if unsafe { IsZoomed(target) }.as_bool() { SC_RESTORE } else { SC_MAXIMIZE }),
            _ => None,
        };
        if let Some(cmd) = command { unsafe {
            if cmd == SC_MINIMIZE {
                // Remove the static overlay before the Genie ghost is shown. The
                // shared caption stays alive until the target has minimized.
                for raw in [self.base.hwnd(), self.lights.hwnd(), self.mask.hwnd()] { let _ = ShowWindow(HWND(raw as *mut _), SW_HIDE); }
            }
            let _ = PostMessageW(Some(target), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0));
        } }
    }

    fn publish(&self, lights: &[u32]) {
        let Some(shared) = &self.shared else { return };
        let r = self.lights.rect();
        if lights.len() != (r.w * r.h) as usize { return; }
        let mut image = self.base_image.clone();
        let mut layer = vec![0u32; image.pixels.len()];
        for y in 0..r.h as usize {
            let source = &lights[y * r.w as usize..(y + 1) * r.w as usize];
            let start = y * image.width as usize;
            layer[start..start + r.w as usize].copy_from_slice(source);
        }
        glass::cpu::composite_over(&mut image, &layer);
        let _ = shared.publish(image.height as usize, &image.pixels);
    }
}

/// Isolated one-window prototype, not started by macshell itself.
pub struct CaptionProbe { tid:u32, thread:Option<JoinHandle<()>> }
impl CaptionProbe {
    pub fn start_for(target:isize) -> Option<Self> {
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().name("macshell-caption-probe".into()).spawn(move || unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            let timer = SetTimer(None, 0, 35, None);
            if timer == 0 { let _ = tx.send(0); return; }
            let _ = tx.send(GetCurrentThreadId());
            let target = HWND(target as *mut _);
            let mut overlay:Option<Overlay> = None;
            // Retain the sampled surface across a resize or minimize animation:
            // the desktop can briefly show through while DWM is moving the target.
            let mut sampled:Option<[u8; 3]> = None;
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg); DispatchMessageW(&msg);
                if !IsWindow(Some(target)).as_bool() { break; }
                let foreground = GetForegroundWindow();
                let active = foreground == target || overlay.as_ref().is_some_and(|o| o.owns(foreground));
                if !active { overlay = None; continue; }
                let Some(info) = caption_info(target) else { overlay = None; continue; };
                if overlay.as_mut().is_some_and(|o| !o.move_to(info.clone())) { overlay = None; }
                if overlay.is_none() {
                    let color = *sampled.get_or_insert_with(|| sample_color(info.rect));
                    overlay = Overlay::open(target, info, color);
                }
                if let Some(ref o) = overlay { o.input(target); }
            }
            drop(overlay); let _ = KillTimer(None, timer);
        }).ok()?;
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(tid) if tid != 0 => Some(Self { tid, thread:Some(thread) }),
            _ => None,
        }
    }
}
impl Drop for CaptionProbe {
    fn drop(&mut self) {
        unsafe { let _ = PostThreadMessageW(self.tid, WM_QUIT, WPARAM(0), LPARAM(0)); }
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}
