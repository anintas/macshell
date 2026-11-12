//! Placement of a single application window within its monitor's work area.
use capture::RectI;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, IsIconic, IsWindow, IsZoomed, SetWindowPos, ShowWindowAsync, GWL_STYLE,
    SW_RESTORE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOZORDER, WS_THICKFRAME,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout { Fill, Left, Right, Top, Bottom, TopLeft, TopRight, BottomLeft, BottomRight }

/// Divide a work area without losing pixels at odd widths or on negative-coordinate monitors.
pub fn rectangle(area: RectI, layout: Layout, gap: i32) -> RectI {
    let gap = gap.max(0).min(area.w.max(0) / 4).min(area.h.max(0) / 4);
    let x0 = area.x + gap;
    let y0 = area.y + gap;
    let x2 = area.x + area.w - gap;
    let y2 = area.y + area.h - gap;
    let mid_x = area.x + area.w / 2;
    let mid_y = area.y + area.h / 2;
    let (left, right) = (mid_x - gap / 2, mid_x + (gap + 1) / 2);
    let (top, bottom) = (mid_y - gap / 2, mid_y + (gap + 1) / 2);
    let (x, y, r, b) = match layout {
        Layout::Fill => (x0, y0, x2, y2),
        Layout::Left => (x0, y0, left, y2),
        Layout::Right => (right, y0, x2, y2),
        Layout::Top => (x0, y0, x2, top),
        Layout::Bottom => (x0, bottom, x2, y2),
        Layout::TopLeft => (x0, y0, left, top),
        Layout::TopRight => (right, y0, x2, top),
        Layout::BottomLeft => (x0, bottom, left, y2),
        Layout::BottomRight => (right, bottom, x2, y2),
    };
    RectI { x, y, w: (r-x).max(0), h: (b-y).max(0) }
}

/// Non-resizable windows are left untouched. The work area is monitor-specific.
pub fn place(target: isize, layout: Layout) -> bool {
    let hwnd = HWND(target as *mut _);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME.0 == 0 { return false; }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() { return false; }
        let r = info.rcWork;
        place_in_area(target, layout, RectI { x: r.left, y: r.top, w: r.right-r.left, h: r.bottom-r.top })
    }
}

/// Place using an explicitly selected monitor, such as the one under a dragged pointer.
pub fn place_in_area(target: isize, layout: Layout, area: RectI) -> bool {
    let hwnd = HWND(target as *mut _);
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
    place_in_area_with_dpi(target, layout, area, dpi)
}

pub fn place_in_area_with_dpi(target: isize, layout: Layout, area: RectI, dpi: u32) -> bool {
    let hwnd = HWND(target as *mut _);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME.0 == 0 { return false; }
        let rect = rectangle(area, layout, (8 * dpi.max(96) as i32 + 48) / 96);
        if rect.w <= 0 || rect.h <= 0 { return false; }
        if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() { let _ = ShowWindowAsync(hwnd, SW_RESTORE); }
        SetWindowPos(hwnd, None, rect.x, rect.y, rect.w, rect.h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn halves_and_quarters_respect_work_area_and_gaps_on_negative_monitors() {
        let area = RectI { x: -1921, y: 27, w: 1921, h: 1053 };
        let l = rectangle(area, Layout::Left, 8);
        let r = rectangle(area, Layout::Right, 8);
        let tl = rectangle(area, Layout::TopLeft, 8);
        let bl = rectangle(area, Layout::BottomLeft, 8);
        assert_eq!(l.x, area.x + 8);
        assert_eq!(r.x - (l.x + l.w), 8);
        assert_eq!(r.x + r.w, area.x + area.w - 8);
        assert_eq!(bl.y - (tl.y + tl.h), 8);
        assert_eq!(bl.y + bl.h, area.y + area.h - 8);
    }

    #[test]
    fn moves_a_real_resizable_test_window_and_leaves_fixed_window_untouched() {
        use windows::core::w;
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, GetWindowRect, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_POPUP};
        unsafe {
            let make = |style| CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("macshell layout test"), style,
                120, 120, 320, 240, None, None, None, None).unwrap();
            let hwnd = make(WS_OVERLAPPEDWINDOW);
            assert!(place(hwnd.0 as isize, Layout::Left));
            let mut actual = RECT::default();
            GetWindowRect(hwnd, &mut actual).unwrap();
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            assert!(GetMonitorInfoW(monitor, &mut info).as_bool());
            let area = info.rcWork;
            let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
            let expected = rectangle(RectI { x: area.left, y: area.top, w: area.right-area.left, h: area.bottom-area.top }, Layout::Left, (8 * dpi as i32 + 48) / 96);
            assert_eq!((actual.left, actual.top, actual.right-actual.left, actual.bottom-actual.top), (expected.x, expected.y, expected.w, expected.h));
            DestroyWindow(hwnd).unwrap();
            let fixed = make(WS_POPUP);
            assert!(!place(fixed.0 as isize, Layout::Right));
            DestroyWindow(fixed).unwrap();
        }
    }
}
