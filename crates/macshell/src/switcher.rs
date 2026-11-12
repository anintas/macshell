//! Переключатель приложений в духе Cmd+Tab. Alt+Tab перехватывает хук клавиатуры (`winkey.rs`): пока Alt держат,
//! Tab и Shift+Tab двигают выбор, стрелки тоже, Esc отменяет, Q закрывает выбранное приложение, H прячет его,
//! отпускание Alt переключает. Быстрое Alt+Tab без задержки переключает на прошлое приложение и ничего не
//! рисует: окно с рядом значков появляется, только если Alt держат дольше `SHOW_AFTER`. Приложения идут в порядке
//! «последнее использованное первым» (порядок окон по Z), у приложения с несколькими окнами берётся верхнее.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use capture::RectI;
use glass::{Bitmap, cpu};
use slint::{ModelRc, VecModel};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::bar::popup_glass;
use crate::dock::AppWindow;
use crate::host::{InputEvent, LayeredWindow, primary_scale};
use crate::icons::{IconPixels, load_icon};
use crate::index::Entry as IndexEntry;
use crate::popups::popup_glass_params;
use crate::slint_view::{ContentView, SwitchTile, Switcher, image_from_pixels};

/// Сколько держать Alt, чтобы показался ряд значков.
pub const SHOW_AFTER: Duration = Duration::from_millis(180);
const PAD: f32 = 18.0;
const GAP: f32 = 8.0;
const MAX_TILE: f32 = 96.0;
const MIN_TILE: f32 = 56.0;
/// Больше приложений в ряд не помещается даже с самыми мелкими плитками.
const MAX_ENTRIES: usize = 24;

/// Приложение в переключателе: все его окна (верхнее первым).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub target: String,
    pub name: String,
    pub hwnds: Vec<isize>,
}

/// Окна в порядке Z (сверху вниз) в приложения в порядке первого окна.
pub fn group(windows: &[AppWindow]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for w in windows {
        match out.iter_mut().find(|e| e.key == w.key) {
            Some(e) => e.hwnds.push(w.hwnd),
            None => out.push(Entry { key: w.key.clone(), target: w.target.clone(), name: w.title.clone(), hwnds: vec![w.hwnd] }),
        }
    }
    out.truncate(MAX_ENTRIES);
    out
}

/// Название приложения по списку установленных (у упакованных приложений заголовок окна это, например, название
/// чата, а не приложения).
pub fn name_from_index(target: &str, known: &[IndexEntry]) -> Option<String> {
    known.iter().find(|k| k.target.eq_ignore_ascii_case(target)).map(|k| k.title.clone())
}

/// Приложения сейчас: название берётся из списка установленных, у запущенных из exe ещё и из ресурсов файла;
/// иначе остаётся заголовок окна.
pub fn entries(known: &[IndexEntry]) -> Vec<Entry> {
    let mut list = group(&crate::winlist::list());
    for e in &mut list {
        let packaged = e.target.to_lowercase().starts_with("shell:appsfolder");
        let name = name_from_index(&e.target, known).or_else(|| if packaged { None } else { crate::foreground::resource_name(&e.target) });
        if let Some(name) = name {
            e.name = name;
        }
    }
    list
}

/// С какого приложения начинается выбор при первом Tab: следующее по давности (или предыдущее, если Shift).
pub fn first_index(len: usize, backwards: bool) -> usize {
    match (len, backwards) {
        (0 | 1, _) => 0,
        (_, false) => 1,
        (n, true) => n - 1,
    }
}

/// Шаг выбора по кругу.
pub fn step(selected: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    (selected as i32 + delta).rem_euclid(len as i32) as usize
}

/// Размер плитки и окна в логических пикселях для `n` приложений на экране шириной `screen_w` (тоже логических).
pub fn layout(n: usize, screen_w: f32) -> (f32, f32, f32) {
    let n = n.max(1) as f32;
    let room = screen_w * 0.92 - 2.0 * PAD - GAP * (n - 1.0);
    let tile = (room / n).clamp(MIN_TILE, MAX_TILE).floor();
    (tile, 2.0 * PAD + n * tile + (n - 1.0) * GAP, tile + 62.0)
}

/// Номер плитки под точкой окна (логические пиксели).
pub fn tile_at(x: f32, y: f32, n: usize, tile: f32) -> Option<usize> {
    if y < PAD || y > PAD + tile + 40.0 || x < PAD {
        return None;
    }
    let slot = ((x - PAD) / (tile + GAP)).floor() as usize;
    (slot < n && (x - PAD) - slot as f32 * (tile + GAP) <= tile).then_some(slot)
}

thread_local! {
    static ICONS: RefCell<HashMap<(String, i32), Option<IconPixels>>> = RefCell::new(HashMap::new());
}

fn icon_for(target: &str, px: i32) -> Option<IconPixels> {
    ICONS.with(|c| c.borrow_mut().entry((target.to_string(), px)).or_insert_with(|| load_icon(target, px)).clone())
}

/// Что сделала мышь в окне.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Hover(usize),
    Pick(usize),
}

pub struct SwitcherWin {
    win: LayeredWindow,
    view: ContentView<Switcher>,
    glass: Bitmap,
    scale: f32,
    count: usize,
    tile: f32,
    opened: Instant,
    shown: usize,
}

impl SwitcherWin {
    pub fn open(entries: &[Entry], selected: usize) -> Result<SwitcherWin> {
        let scale = primary_scale();
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let (tile, w_pt, h_pt) = layout(entries.len(), sw as f32 / scale);
        let (w, h) = ((w_pt * scale).round() as i32, (h_pt * scale).round() as i32);
        let rect = RectI { x: (sw - w) / 2, y: (sh - h) / 2, w, h };
        let mut win = LayeredWindow::new(rect)?;
        win.animate_overlay(true, (6.0 * scale).round() as i32);
        let glass = popup_glass(win.rect(), &popup_glass_params().scaled(scale))?;
        let view = ContentView::new_with(w as u32, h as u32, Switcher::new);
        view.set_scale_factor(scale);
        let px = (tile * 0.67 * scale).round() as i32;
        let tiles: Vec<SwitchTile> = entries
            .iter()
            .map(|e| {
                let icon = icon_for(&e.target, px);
                SwitchTile { has_icon: icon.is_some(), icon: icon.as_ref().map(image_from_pixels).unwrap_or_default(), name: e.name.as_str().into() }
            })
            .collect();
        view.ui.set_tiles(ModelRc::new(VecModel::from(tiles)));
        view.ui.set_tile(tile);
        view.ui.set_selected(selected as i32);
        let this = SwitcherWin { win, view, glass, scale, count: entries.len(), tile, opened: Instant::now(), shown: selected };
        this.present(true)?;
        Ok(this)
    }

    /// Выбор показывает другое приложение.
    pub fn select(&mut self, i: usize) {
        if i != self.shown {
            self.shown = i;
            self.view.ui.set_selected(i as i32);
        }
    }

    /// Мышь: наведение выбирает, нажатие переключает. Первые 300 мс наведение не считается: окно возникает под
    /// курсором, и система шлёт движение, которого пользователь не делал.
    pub fn mouse(&self) -> Option<Mouse> {
        let mut out = None;
        for ev in self.win.take_input() {
            let at = |x: i32, y: i32| tile_at(x as f32 / self.scale, y as f32 / self.scale, self.count, self.tile);
            match ev {
                InputEvent::Move { x, y } if self.opened.elapsed() > Duration::from_millis(300) => {
                    if let Some(i) = at(x, y) {
                        out = Some(Mouse::Hover(i));
                    }
                }
                InputEvent::Down { x, y } => {
                    if let Some(i) = at(x, y) {
                        out = Some(Mouse::Pick(i));
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub fn present(&self, force: bool) -> Result<()> {
        let content = match self.view.render_if_changed() {
            Some(c) => c,
            None if force => self.view.render(),
            None => return Ok(()),
        };
        let mut out = self.glass.clone();
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

    fn win(hwnd: isize, key: &str, title: &str) -> AppWindow {
        AppWindow { hwnd, key: key.into(), target: format!("c:\\{key}.exe"), title: title.into() }
    }

    #[test]
    fn windows_group_into_apps_in_order_of_the_top_window() {
        let list = group(&[win(1, "a", "A1"), win(2, "b", "B1"), win(3, "a", "A2"), win(4, "c", "C1")]);
        assert_eq!(list.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["a", "b", "c"]);
        assert_eq!(list[0].hwnds, vec![1, 3], "the top window comes first");
        assert_eq!(list[0].name, "A1");
    }

    #[test]
    fn packaged_apps_are_named_by_the_installed_list_not_by_the_window_title() {
        let known = vec![IndexEntry {
            title: "AyuGram".into(),
            norm: "ayugram".into(),
            target: r"shell:AppsFolder\Ayu.Gram".into(),
            keywords: Vec::new(),
            kind: crate::index::Kind::App,
            glyph: "",
        }];
        assert_eq!(name_from_index(r"shell:appsfolder\ayu.gram", &known).as_deref(), Some("AyuGram"));
        assert_eq!(name_from_index(r"c:\none.exe", &known), None);
    }

    #[test]
    fn the_first_tab_picks_the_previous_app() {
        assert_eq!((first_index(0, false), first_index(1, false), first_index(5, false)), (0, 0, 1));
        assert_eq!(first_index(5, true), 4, "Shift+Tab goes to the oldest");
    }

    #[test]
    fn selection_wraps_both_ways() {
        assert_eq!((step(4, 5, 1), step(0, 5, -1), step(2, 5, 1), step(0, 0, 1)), (0, 4, 3, 0));
    }

    #[test]
    fn tiles_shrink_to_fit_the_screen_and_stay_in_bounds() {
        let (tile, w, h) = layout(5, 1920.0);
        assert_eq!(tile, MAX_TILE);
        assert_eq!((w, h), (36.0 + 5.0 * 96.0 + 4.0 * 8.0, 158.0));
        let (tile, w, _) = layout(24, 1920.0);
        assert!(tile < MAX_TILE && tile >= MIN_TILE && w <= 1920.0 * 0.92 + 1.0, "{tile} {w}");
        assert_eq!(layout(0, 1920.0), layout(1, 1920.0));
    }

    #[test]
    fn a_point_finds_its_tile_and_gaps_find_none() {
        let tile = 96.0;
        assert_eq!(tile_at(18.0 + 10.0, 40.0, 3, tile), Some(0));
        assert_eq!(tile_at(18.0 + 96.0 + 8.0 + 5.0, 40.0, 3, tile), Some(1));
        assert_eq!(tile_at(18.0 + 96.0 + 3.0, 40.0, 3, tile), None, "gap");
        assert_eq!(tile_at(18.0 + 3.0 * 104.0 + 1.0, 40.0, 3, tile), None, "past the last tile");
        assert_eq!(tile_at(40.0, 5.0, 3, tile), None, "above the row");
    }

    #[test]
    #[ignore = "opens a real window on the desktop"]
    fn window_opens_with_real_apps() {
        let list = entries(&[]);
        if list.is_empty() {
            return;
        }
        let mut w = SwitcherWin::open(&list, first_index(list.len(), false)).expect("open");
        w.select(0);
        w.present(false).expect("present");
        assert!(w.mouse().is_none());
    }
}
