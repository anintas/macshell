//! Системная панель (AppBar) на верхнем или нижнем краю: окна приложений не заходят под неё.

use anyhow::{Result, bail};
use capture::RectI;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA, QUNS_PRESENTATION_MODE,
    QUNS_RUNNING_D3D_FULL_SCREEN, SHAppBarMessage, SHQueryUserNotificationState,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, HWND_BOTTOM, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SetWindowPos,
};

/// Сейчас работает игра в полноэкранном режиме Direct3D или идёт показ презентации. Только тогда сочетания клавиш
/// (Win, Alt+Tab) отдаются системе: в обычном окне «на весь экран» (F11, видео, это приложение) они работают как всегда.
pub fn exclusive_fullscreen() -> bool {
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

pub struct AppBar {
    hwnd: HWND,
    callback_msg: u32,
    edge: Edge,
    rc: RECT,
    /// Монитор, у края которого стоит панель; `None` значит основной.
    bounds: Option<RECT>,
}

fn data(hwnd: HWND, callback_msg: u32) -> APPBARDATA {
    APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: callback_msg,
        ..Default::default()
    }
}

/// Занятая полоса: у верхнего края `size` пикселей вниз от `top`, у нижнего `size` пикселей вверх от `bottom`.
pub fn claimed(edge: Edge, top: i32, bottom: i32, size: i32) -> (i32, i32) {
    match edge {
        Edge::Top => (top, top + size),
        Edge::Bottom => (bottom - size, bottom),
    }
}

impl AppBar {
    /// Занимает полосу высотой `height_px` сверху основного монитора. Положение окна вызывающий
    /// выставляет сам по `rect()`.
    pub fn register(hwnd: isize, callback_msg: u32, height_px: i32) -> Result<AppBar> {
        AppBar::register_at(hwnd, callback_msg, Edge::Top, height_px)
    }

    /// Занимает полосу у верхнего края другого монитора (`monitor`: его прямоугольник на рабочем столе).
    pub fn register_on(hwnd: isize, callback_msg: u32, size_px: i32, monitor: RectI) -> Result<AppBar> {
        let bounds = RECT { left: monitor.x, top: monitor.y, right: monitor.x + monitor.w, bottom: monitor.y + monitor.h };
        AppBar::register_inner(hwnd, callback_msg, Edge::Top, size_px, Some(bounds))
    }

    pub fn register_at(hwnd: isize, callback_msg: u32, edge: Edge, size_px: i32) -> Result<AppBar> {
        AppBar::register_inner(hwnd, callback_msg, edge, size_px, None)
    }

    fn register_inner(hwnd: isize, callback_msg: u32, edge: Edge, size_px: i32, bounds: Option<RECT>) -> Result<AppBar> {
        let hwnd = HWND(hwnd as *mut _);
        let mut abd = data(hwnd, callback_msg);
        if unsafe { SHAppBarMessage(ABM_NEW, &mut abd) } == 0 {
            bail!("ABM_NEW refused: another appbar owns this window?");
        }
        let mut bar = AppBar { hwnd, callback_msg, edge, rc: RECT::default(), bounds };
        bar.reposition(size_px)?;
        Ok(bar)
    }

    /// Пересчитывает полосу после смены разрешения, размера или чужой панели. `true`, если место изменилось.
    /// Если прямоугольник тот же, `ABM_SETPOS` не шлётся: иначе система отвечает `ABN_POSCHANGED` и цикл не кончится.
    pub fn reposition(&mut self, size_px: i32) -> Result<bool> {
        let mut abd = data(self.hwnd, self.callback_msg);
        abd.uEdge = match self.edge {
            Edge::Top => ABE_TOP,
            Edge::Bottom => ABE_BOTTOM,
        };
        abd.rc = self.bounds.unwrap_or(RECT {
            left: 0,
            top: 0,
            right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
            bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
        });
        let edge = self.edge;
        // Результат ABM_QUERYPOS и ABM_SETPOS приходит через общую память Проводника: хост трея (если он есть)
        // на это время опускается за окно Проводника.
        let same = crate::tray::host::with_host_lowered(|| {
            unsafe {
                SHAppBarMessage(ABM_QUERYPOS, &mut abd);
                (abd.rc.top, abd.rc.bottom) = claimed(edge, abd.rc.top, abd.rc.bottom, size_px);
            }
            let same = abd.rc == self.rc;
            if !same {
                unsafe { SHAppBarMessage(ABM_SETPOS, &mut abd) };
            }
            same
        });
        if !same {
            self.rc = abd.rc;
        }
        Ok(!same)
    }

    pub fn rect(&self) -> RectI {
        RectI { x: self.rc.left, y: self.rc.top, w: self.rc.right - self.rc.left, h: self.rc.bottom - self.rc.top }
    }

    /// Приложение на весь экран: полоса уходит под все окна, потом возвращается наверх.
    pub fn set_fullscreen(&self, on: bool) {
        let after = if on { HWND_BOTTOM } else { HWND_TOPMOST };
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

impl Drop for AppBar {
    fn drop(&mut self) {
        let mut abd = data(self.hwnd, self.callback_msg);
        unsafe { SHAppBarMessage(ABM_REMOVE, &mut abd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_edge_claims_downwards_and_bottom_edge_upwards() {
        assert_eq!(claimed(Edge::Top, 0, 1080, 24), (0, 24));
        assert_eq!(claimed(Edge::Bottom, 24, 1080, 80), (1000, 1080));
        // Нижний край уже поджат другой панелью: берём от её верха.
        assert_eq!(claimed(Edge::Bottom, 24, 1040, 80), (960, 1040));
    }
}
