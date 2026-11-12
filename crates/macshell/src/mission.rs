//! Mission Control: затемнённые обои, сверху ряд рабочих столов, ниже все окна плитками с живым содержимым на
//! момент открытия. Миниатюры снимает `PrintWindow` по одной за кадр, чтобы обзор открывался сразу, а плитки
//! наполнялись на глазах. Щелчок по плитке выносит окно вперёд, по столу переключает его, «+» создаёт новый.
//! Рабочие столы Windows читаются из реестра (число и текущий), переключаются сочетаниями Ctrl+Win+стрелки.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use capture::RectI;
use glass::{Bitmap, cpu};
use slint::{Image, Model, ModelRc, VecModel};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, HGDIOBJ,
    SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, GetWindowRect, IsIconic, SM_CXSCREEN, SM_CYSCREEN};
use windows::core::w;

use crate::bar_style::SoftBackdrop;
use crate::dock::AppWindow;
use crate::host::{InputEvent, LayeredWindow, primary_scale};
use crate::icons::load_icon;
use crate::slint_view::{ContentView, DesktopTile, MissionControl, MissionTile, image_from_pixels};

const VK_ESCAPE: u32 = 0x1B;
/// Окон в обзоре не больше: больше плиток всё равно не разглядеть.
const MAX_WINDOWS: usize = 18;
/// Подпись под миниатюрой, логические пиксели.
const LABEL: f32 = 30.0;
const GAP: f32 = 22.0;
/// Миниатюры больше этой ширины (физические пиксели) уменьшаются: плитка на экране меньше.
const MAX_THUMB_W: u32 = 760;
const FADE_MS: u64 = 160;

/// Раскладка плиток: колонки, строки, размер ячейки. Берётся число колонок, при котором миниатюры крупнее всего
/// (окна считаются 16:10).
pub fn grid(n: usize, w: f32, h: f32) -> (usize, usize, f32, f32) {
    let n = n.max(1);
    let mut best = (1, n, w, h, 0.0f32);
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let cw = (w - GAP * (cols as f32 - 1.0)) / cols as f32;
        let ch = (h - GAP * (rows as f32 - 1.0)) / rows as f32;
        let thumb_h = (ch - LABEL).max(1.0);
        let scale = (cw / 1.6).min(thumb_h);
        if scale > best.4 {
            best = (cols, rows, cw, ch, scale);
        }
    }
    (best.0, best.1, best.2, best.3)
}

/// Место плитки `index` из `n` в области `area` (x, y, ширина, высота) для окна с отношением сторон `aspect`
/// (ширина к высоте): окно вписывается в ячейку и стоит по центру. Возвращает (x, y, ширина, высота) плитки вместе
/// с подписью.
pub fn tile_rect(index: usize, n: usize, area: (f32, f32, f32, f32), aspect: f32) -> (f32, f32, f32, f32) {
    let (cols, rows, cw, ch) = grid(n, area.2, area.3);
    let (col, row) = (index % cols, index / cols);
    // Последний ряд, если он неполный, центруется.
    let in_row = if row == rows - 1 { n - (rows - 1) * cols } else { cols };
    let shift = (cols - in_row) as f32 * (cw + GAP) / 2.0;
    let (cx, cy) = (area.0 + shift + col as f32 * (cw + GAP), area.1 + row as f32 * (ch + GAP));
    let thumb_h = ch - LABEL;
    let aspect = aspect.clamp(0.4, 3.5);
    let (tw, th) = if cw / thumb_h > aspect { (thumb_h * aspect, thumb_h) } else { (cw, cw / aspect) };
    (cx + (cw - tw) / 2.0, cy + (thumb_h - th) / 2.0, tw, th + LABEL)
}

/// Число рабочих столов и номер текущего по значениям реестра: `ids` это подряд идущие GUID по 16 байт.
pub fn parse_desktops(ids: &[u8], current: &[u8]) -> (usize, usize) {
    let count = (ids.len() / 16).max(1);
    let index = if current.len() >= 16 { ids.chunks_exact(16).position(|c| c == &current[..16]).unwrap_or(0) } else { 0 };
    (count, index)
}

fn read_binary(key: &windows::core::PCWSTR, name: &windows::core::PCWSTR) -> Vec<u8> {
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, *key, *name, RRF_RT_REG_BINARY, None, None, Some(&mut size)).is_err() || size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size as usize];
        if RegGetValueW(HKEY_CURRENT_USER, *key, *name, RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size)).is_err() {
            return Vec::new();
        }
        buf.truncate(size as usize);
        buf
    }
}

/// (число столов, номер текущего) по реестру; один стол, если ключа нет.
pub fn desktops() -> (usize, usize) {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops");
    let ids = read_binary(&key, &w!("VirtualDesktopIDs"));
    let mut current = read_binary(&key, &w!("CurrentVirtualDesktop"));
    if current.is_empty() {
        let session = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\SessionInfo\\1\\VirtualDesktops");
        current = read_binary(&session, &w!("CurrentVirtualDesktop"));
    }
    parse_desktops(&ids, &current)
}

/// Что обзор просит сделать после закрытия.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Close,
    Activate(isize),
    /// Перейти на стол с этим номером (от нуля).
    Desktop(usize),
    NewDesktop,
}

/// Снимок окна: RGBA, ширина, высота. `PrintWindow` с полным содержимым берёт и окна на GPU.
pub(crate) fn snapshot(hwnd: isize) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let hwnd = HWND(hwnd as *mut _);
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        let (w, h) = ((r.right - r.left).max(0), (r.bottom - r.top).max(0));
        if w < 8 || h < 8 {
            return None;
        }
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        let result = match dib {
            Some(dib) if !bits.is_null() => {
                let old = SelectObject(dc, HGDIOBJ(dib.0));
                let ok = PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(2)).as_bool();
                let _ = GdiFlush();
                let px = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize);
                let out = ok.then(|| shrink(px, w as u32, h as u32));
                SelectObject(dc, old);
                let _ = DeleteObject(HGDIOBJ(dib.0));
                out
            }
            _ => None,
        };
        let _ = DeleteDC(dc);
        result
    }
}

/// BGRA в RGBA без прозрачности и, если широко, уменьшение целым числом раз усреднением блоков.
fn shrink(px: &[u32], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let f = w.div_ceil(MAX_THUMB_W).max(1);
    let (tw, th) = (w / f, h / f);
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        for x in 0..tw {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for dy in 0..f {
                for dx in 0..f {
                    let p = px[((y * f + dy) * w + x * f + dx) as usize];
                    r += (p >> 16) & 0xFF;
                    g += (p >> 8) & 0xFF;
                    b += p & 0xFF;
                }
            }
            let n = f * f;
            out.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8, 255]);
        }
    }
    (out, tw, th)
}

struct Item {
    hwnd: isize,
    iconic: bool,
}

pub struct MissionWin {
    win: LayeredWindow,
    view: ContentView<MissionControl>,
    bg: Bitmap,
    tiles: Rc<VecModel<MissionTile>>,
    items: Vec<Item>,
    pending: RefCell<VecDeque<usize>>,
    picked: Rc<Cell<i32>>,
    desk: Rc<Cell<i32>>,
    add: Rc<Cell<bool>>,
    bg_click: Rc<Cell<bool>>,
    desks: (usize, usize),
    opened: Instant,
    motion: bool,
    previous: isize,
    scale: f32,
    area: (f32, f32, f32, f32),
}

/// Обои, размытые и затемнённые, как фон обзора; без файла обоев просто тёмный фон.
fn backdrop(sw: i32, sh: i32) -> Bitmap {
    let mut bg = match crate::wallpaper::screen(sw as u32, sh as u32) {
        Some(wall) => {
            let blurred = SoftBackdrop::new(&wall, 26);
            blurred.region(0, 0, sw as u32, sh as u32)
        }
        None => {
            let mut b = Bitmap::new(sw as u32, sh as u32);
            b.pixels.fill(0xFF202228);
            b
        }
    };
    for p in bg.pixels.iter_mut() {
        let c = |v: u32| (v * 11 / 25).min(255);
        *p = 0xFF00_0000 | (c((*p >> 16) & 0xFF) << 16) | (c((*p >> 8) & 0xFF) << 8) | c(*p & 0xFF);
    }
    bg
}

impl MissionWin {
    pub fn open(windows: &[AppWindow]) -> Result<MissionWin> {
        let scale = primary_scale();
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let previous = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize };
        let mut win = LayeredWindow::new_focusable(RectI { x: 0, y: 0, w: sw, h: sh })?;
        let motion = crate::anim::enabled();
        win.animate_overlay(false, 0);
        if motion { win.set_blend(0, 0)?; }
        let view = ContentView::new_with(sw as u32, sh as u32, MissionControl::new);
        view.set_scale_factor(scale);
        let (lw, lh) = (sw as f32 / scale, sh as f32 / scale);
        let area = (64.0, 124.0, lw - 128.0, lh - 124.0 - 56.0);
        let list: Vec<&AppWindow> = windows.iter().take(MAX_WINDOWS).collect();
        let n = list.len();
        let px = (28.0 * scale).round() as i32;
        let mut items = Vec::new();
        let tiles: Vec<MissionTile> = list
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let (x, y, tw, th) = tile_rect(i, n, area, 1.6);
                let iconic = unsafe { IsIconic(HWND(w.hwnd as *mut _)) }.as_bool();
                items.push(Item { hwnd: w.hwnd, iconic });
                let icon = load_icon(&w.target, px);
                MissionTile {
                    image: Image::default(),
                    has_image: false,
                    has_icon: icon.is_some(),
                    icon: icon.as_ref().map(image_from_pixels).unwrap_or_default(),
                    title: w.title.as_str().into(),
                    x,
                    y,
                    w: tw,
                    h: th,
                }
            })
            .collect();
        let model = Rc::new(VecModel::from(tiles));
        view.ui.set_tiles(ModelRc::from(model.clone()));
        let desks = desktops();
        let names: Vec<DesktopTile> = (0..desks.0).map(|i| DesktopTile { name: format!("Рабочий стол {}", i + 1).into(), current: i == desks.1 }).collect();
        view.ui.set_desktops(ModelRc::new(VecModel::from(names)));
        view.ui.set_can_add_desktop(desks.0 < 10);
        let picked = Rc::new(Cell::new(-1));
        let slot = picked.clone();
        view.ui.on_picked(move |i| slot.set(i));
        let desk = Rc::new(Cell::new(-1));
        let slot = desk.clone();
        view.ui.on_desktop_picked(move |i| slot.set(i));
        let add = Rc::new(Cell::new(false));
        let slot = add.clone();
        view.ui.on_desktop_added(move || slot.set(true));
        let bg_click = Rc::new(Cell::new(false));
        let slot = bg_click.clone();
        view.ui.on_background_clicked(move || slot.set(true));
        let pending: VecDeque<usize> = (0..n).filter(|i| !items[*i].iconic).collect();
        let this = MissionWin {
            bg: backdrop(sw, sh),
            win,
            view,
            tiles: model,
            items,
            pending: RefCell::new(pending),
            picked,
            desk,
            add,
            bg_click,
            desks,
            opened: Instant::now(),
            motion,
            previous,
            scale,
            area,
        };
        this.present(true)?;
        this.win.focus();
        Ok(this)
    }

    fn present(&self, force: bool) -> Result<()> {
        let content = match self.view.render_if_changed() {
            Some(c) => c,
            None if force => self.view.render(),
            None => return Ok(()),
        };
        let mut out = self.bg.clone();
        cpu::composite_over(&mut out, &content);
        self.win.present(&out)
    }

    /// Один шаг: появление, две новые миниатюры, ввод. Возвращает, что делать дальше.
    pub fn tick(&self) -> Result<Outcome> {
        let t = (self.opened.elapsed().as_millis() as f32 / FADE_MS as f32).clamp(0.0, 1.0);
        if self.motion && (t < 1.0 || self.opened.elapsed() < Duration::from_millis(FADE_MS + 40)) {
            self.win.set_blend((255.0 * crate::anim::ease_out_cubic(t)).round() as u8, 0)?;
        }
        let mut outcome = Outcome::Stay;
        for ev in self.win.take_input() {
            match ev {
                InputEvent::Key { vk: VK_ESCAPE, .. } => outcome = Outcome::Close,
                other => self.view.dispatch(&other),
            }
        }
        for _ in 0..2 {
            let Some(i) = self.pending.borrow_mut().pop_front() else { break };
            if let Some((rgba, w, h)) = snapshot(self.items[i].hwnd) {
                let image = Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h));
                if let Some(mut row) = self.tiles.row_data(i) {
                    // Плитка принимает форму окна: рамка по его отношению сторон.
                    let (x, y, tw, th) = tile_rect(i, self.items.len(), self.area, w as f32 / h as f32);
                    (row.image, row.has_image, row.x, row.y, row.w, row.h) = (image, true, x, y, tw, th);
                    self.tiles.set_row_data(i, row);
                }
            }
        }
        let picked = self.picked.replace(-1);
        if picked >= 0 {
            if let Some(it) = self.items.get(picked as usize) {
                outcome = Outcome::Activate(it.hwnd);
            }
        }
        let desk = self.desk.replace(-1);
        if desk >= 0 {
            outcome = if desk as usize == self.desks.1 { Outcome::Close } else { Outcome::Desktop(desk as usize) };
        }
        if self.add.replace(false) {
            outcome = Outcome::NewDesktop;
        }
        if self.bg_click.replace(false) {
            outcome = Outcome::Close;
        }
        self.present(false)?;
        Ok(outcome)
    }

    /// Номер текущего стола и сколько их.
    pub fn desktops(&self) -> (usize, usize) {
        self.desks
    }

    pub fn animating(&self) -> bool {
        !self.pending.borrow().is_empty() || (self.motion && self.opened.elapsed() < Duration::from_millis(FADE_MS + 60)) || self.view.animating()
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }
}

impl Drop for MissionWin {
    fn drop(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};
        let previous = HWND(self.previous as *mut _);
        if self.win.is_foreground() && unsafe { IsWindow(Some(previous)) }.as_bool() {
            unsafe { let _ = SetForegroundWindow(previous); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_picks_columns_that_keep_thumbnails_large() {
        let (cols, rows, cw, ch) = grid(1, 1000.0, 600.0);
        assert_eq!((cols, rows), (1, 1));
        assert_eq!((cw, ch), (1000.0, 600.0));
        let (cols, rows, _, _) = grid(4, 1700.0, 800.0);
        assert_eq!(cols * rows >= 4, true);
        assert!(cols >= 2, "four windows on a wide screen sit side by side: {cols}x{rows}");
        let (cols, rows, _, _) = grid(18, 1700.0, 800.0);
        assert!(cols * rows >= 18 && rows <= 4, "{cols}x{rows}");
    }

    #[test]
    fn a_tile_stays_inside_its_area_and_keeps_the_window_shape() {
        let area = (64.0, 124.0, 1700.0, 800.0);
        for n in [1usize, 2, 5, 9, 18] {
            for i in 0..n {
                let (x, y, w, h) = tile_rect(i, n, area, 1.6);
                assert!(x >= area.0 - 0.5 && x + w <= area.0 + area.2 + 0.5, "n={n} i={i}: x {x} w {w}");
                assert!(y >= area.1 - 0.5 && y + h <= area.1 + area.3 + 0.5, "n={n} i={i}: y {y} h {h}");
                assert!(((w / (h - LABEL)) - 1.6).abs() < 0.02, "n={n} i={i}: aspect {}", w / (h - LABEL));
            }
        }
        let tall = tile_rect(0, 1, area, 0.5);
        assert!(tall.2 < tall.3, "a tall window gets a tall tile");
    }

    #[test]
    fn tiles_do_not_overlap() {
        let area = (0.0, 0.0, 1600.0, 800.0);
        let n = 7;
        let rects: Vec<_> = (0..n).map(|i| tile_rect(i, n, area, 1.6)).collect();
        for a in 0..n {
            for b in a + 1..n {
                let (p, q) = (rects[a], rects[b]);
                let apart = p.0 + p.2 <= q.0 + 0.5 || q.0 + q.2 <= p.0 + 0.5 || p.1 + p.3 <= q.1 + 0.5 || q.1 + q.3 <= p.1 + 0.5;
                assert!(apart, "tiles {a} and {b} overlap: {p:?} {q:?}");
            }
        }
    }

    #[test]
    fn desktops_are_counted_from_guids() {
        let a = [1u8; 16];
        let b = [2u8; 16];
        let ids: Vec<u8> = a.iter().chain(b.iter()).copied().collect();
        assert_eq!(parse_desktops(&ids, &b), (2, 1));
        assert_eq!(parse_desktops(&ids, &a), (2, 0));
        assert_eq!(parse_desktops(&[], &[]), (1, 0), "no key means one desktop");
        assert_eq!(parse_desktops(&ids, &[9u8; 16]), (2, 0), "an unknown current desktop falls back to the first");
    }

    #[test]
    fn shrink_averages_blocks_and_drops_transparency() {
        let px = vec![0x80102030u32; 4 * 4];
        let (rgba, w, h) = shrink(&px, 4, 4);
        assert_eq!((w, h), (4, 4), "small windows are not shrunk");
        assert_eq!(&rgba[..4], &[0x10, 0x20, 0x30, 255]);
        let wide = vec![0xFF00_00FFu32; 1520 * 2];
        let (_, w, h) = shrink(&wide, 1520, 2);
        assert_eq!(w, 760, "a wide window halves");
        assert_eq!(h, 1);
    }

    #[test]
    #[ignore = "opens a real full-screen window on the desktop"]
    fn mission_control_opens_fills_tiles_and_closes() {
        let windows = crate::winlist::list();
        let win = MissionWin::open(&windows).expect("open");
        let start = Instant::now();
        while win.animating() && start.elapsed() < Duration::from_secs(8) {
            assert!(matches!(win.tick().expect("tick"), Outcome::Stay));
        }
        assert!(!win.animating(), "all thumbnails were taken");
        let (count, current) = win.desktops();
        assert!(count >= 1 && current < count);
    }

    #[test]
    fn real_desktops_are_readable() {
        let (count, current) = desktops();
        assert!(count >= 1 && current < count);
    }
}
