//! Панели на дополнительных мониторах: название приложения и часы, как в macOS при включённом «Отдельные
//! пространства». Значки состояния, трей и закреплённые приложения живут только на основной панели. Каждая панель
//! занимает полосу у верхнего края своего монитора, окна приложений на нём не заходят под неё. Фон сплошной
//! (цвет `bar_color`), поэтому текст всегда белый. Высота по масштабу основного монитора.

use std::cell::RefCell;

use anyhow::Result;
use capture::RectI;
use glass::{Bitmap, cpu};
use slint::{ModelRc, VecModel};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::core::BOOL;

use crate::appbar::AppBar;
use crate::bar_style::solid_background;
use crate::host::{InputEvent, LayeredWindow};
use crate::slint_view::{ContentView, TopBar};

const ABN_FULLSCREENAPP: usize = 2;
const ABN_POSCHANGED: usize = 1;
const MSG_APPBAR: u32 = 0x8000 + 20;
const MONITORINFOF_PRIMARY: u32 = 1;

unsafe extern "system" fn visit(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, lparam: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lparam.0 as *mut Vec<RectI>) };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() && info.dwFlags & MONITORINFOF_PRIMARY == 0 {
        let r = info.rcMonitor;
        out.push(RectI { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top });
    }
    true.into()
}

/// Прямоугольники всех мониторов, кроме основного.
pub fn other_monitors() -> Vec<RectI> {
    let mut out: Vec<RectI> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(visit), LPARAM(&mut out as *mut Vec<RectI> as isize));
    }
    out
}

pub struct SecondaryBar {
    // Порядок важен: сначала снимается полоса, потом закрывается окно.
    appbar: AppBar,
    win: LayeredWindow,
    view: ContentView<TopBar>,
    solid: Bitmap,
    height: i32,
    shown: RefCell<(String, String)>,
}

impl SecondaryBar {
    /// Панель высотой `height_px` у верхнего края монитора `monitor`; `scale` перевод логических пикселей в физические.
    pub fn open(monitor: RectI, height_px: i32, scale: f32, color: [u8; 3]) -> Result<SecondaryBar> {
        let win = LayeredWindow::new(RectI { x: monitor.x, y: monitor.y, w: monitor.w, h: height_px })?;
        win.set_title("macshell.bar.secondary");
        let appbar = AppBar::register_on(win.hwnd(), MSG_APPBAR, height_px, monitor)?;
        let r = appbar.rect();
        win.set_position(r.x, r.y)?;
        let view = ContentView::new_with(monitor.w as u32, height_px as u32, TopBar::new);
        view.set_scale_factor(scale);
        view.ui.set_tray_enabled(false);
        view.ui.set_status_order(ModelRc::new(VecModel::from(Vec::<i32>::new())));
        let bar = SecondaryBar {
            appbar,
            win,
            view,
            solid: solid_background(monitor.w as u32, height_px as u32, color),
            height: height_px,
            shown: RefCell::new((String::new(), String::new())),
        };
        bar.present(true)?;
        Ok(bar)
    }

    /// Название приложения и часы с основной панели.
    pub fn sync(&self, app_name: &str, clock: &str) {
        let mut shown = self.shown.borrow_mut();
        if shown.0 != app_name {
            self.view.ui.set_app_name(app_name.into());
            shown.0 = app_name.to_string();
        }
        if shown.1 != clock {
            self.view.ui.set_clock(clock.into());
            shown.1 = clock.to_string();
        }
    }

    /// Мышь для подсветки и сообщения панели (смена места, полноэкранное приложение).
    pub fn handle_input(&mut self) -> Result<()> {
        for ev in self.win.take_input() {
            match ev {
                InputEvent::Message { msg, wparam, lparam } if msg == MSG_APPBAR => match wparam {
                    ABN_FULLSCREENAPP => self.appbar.set_fullscreen(lparam != 0),
                    ABN_POSCHANGED => {
                        if self.appbar.reposition(self.height)? {
                            let r = self.appbar.rect();
                            self.win.set_position(r.x, r.y)?;
                        }
                    }
                    _ => {}
                },
                InputEvent::Message { .. } => {}
                other => self.view.dispatch(&other),
            }
        }
        Ok(())
    }

    pub fn present(&self, force: bool) -> Result<()> {
        let content = match self.view.render_if_changed() {
            Some(c) => c,
            None if force => self.view.render(),
            None => return Ok(()),
        };
        let mut out = self.solid.clone();
        cpu::composite_over(&mut out, &content);
        self.win.present(&out)
    }

    pub fn animating(&self) -> bool {
        self.view.animating()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_other_monitors_does_not_panic_and_excludes_the_primary() {
        let others = other_monitors();
        for m in &others {
            assert!(m.w > 0 && m.h > 0);
        }
    }
}
