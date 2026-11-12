//! Stage Manager (упрощённый): на сцене окна одного приложения, остальные приложения сворачиваются, а их миниатюры
//! стоят полоской у левого края. Щелчок по миниатюре выводит приложение на сцену, прежнее уходит в полоску. Сцена
//! меняется сама, когда на передний план выходит другое приложение (Dock, Alt+Tab, Mission Control).
//! Миниатюра снимается за миг до сворачивания: у свёрнутого окна снимка не взять.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use capture::RectI;
use glass::{Bitmap, cpu};
use slint::{Image, ModelRc, VecModel};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, IsIconic, IsWindow, SM_CYSCREEN, SW_MINIMIZE, SW_SHOWNOACTIVATE, ShowWindow,
};

use crate::dock::AppWindow;
use crate::host::{LayeredWindow, primary_scale};
use crate::icons::load_icon;
use crate::mission::snapshot;
use crate::slint_view::{ContentView, StageStrip, StageTile, image_from_pixels};

/// Сколько приложений показывает полоска.
pub const MAX_SIDE: usize = 6;
const STRIP_W: f32 = 136.0;
const TILE_STEP: f32 = 98.0;

/// Приложение за кулисами: верхнее окно приложения и подпись.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Side {
    pub hwnd: isize,
    pub key: String,
    pub target: String,
    pub title: String,
}

/// Что делать при смене переднего окна: `(сколько окон свернуть, приложения за кулисами)`. Окна идут в порядке Z
/// (верхнее первым), `iconic` у каждого: свёрнуто ли оно. Пусто, если переднего окна среди них нет.
pub fn plan(windows: &[(AppWindow, bool)], front: isize) -> Option<(Vec<isize>, Vec<Side>)> {
    let front_key = &windows.iter().find(|(w, _)| w.hwnd == front)?.0.key;
    let mut minimize = Vec::new();
    let mut side: Vec<Side> = Vec::new();
    for (w, iconic) in windows {
        if &w.key == front_key {
            continue;
        }
        if !iconic {
            minimize.push(w.hwnd);
        }
        if !side.iter().any(|s| s.key == w.key) && side.len() < MAX_SIDE {
            side.push(Side { hwnd: w.hwnd, key: w.key.clone(), target: w.target.clone(), title: w.title.clone() });
        }
    }
    Some((minimize, side))
}

pub struct Stage {
    strip: LayeredWindow,
    view: ContentView<StageStrip>,
    scale: f32,
    side: Vec<Side>,
    shots: HashMap<isize, (Vec<u8>, u32, u32)>,
    picked: Rc<Cell<i32>>,
}

impl Stage {
    pub fn open() -> Result<Stage> {
        let scale = primary_scale();
        let sh = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        let (w, h) = ((STRIP_W * scale).round() as i32, ((MAX_SIDE as f32 * TILE_STEP + 12.0) * scale).round() as i32);
        let mut strip = LayeredWindow::new(RectI { x: (4.0 * scale).round() as i32, y: (sh - h) / 2, w, h })?;
        strip.animate_overlay(true, (6.0 * scale).round() as i32);
        let view = ContentView::new_with(w as u32, h as u32, StageStrip::new);
        view.set_scale_factor(scale);
        let picked = Rc::new(Cell::new(-1));
        let slot = picked.clone();
        view.ui.on_picked(move |i| slot.set(i));
        let stage = Stage { strip, view, scale, side: Vec::new(), shots: HashMap::new(), picked };
        stage.present()?;
        Ok(stage)
    }

    fn present(&self) -> Result<()> {
        let r = self.strip.rect();
        let mut out = Bitmap::new(r.w as u32, r.h as u32);
        cpu::composite_over(&mut out, &self.view.render());
        self.strip.present(&out)
    }

    /// Передний план сменился: приложение `front` выходит на сцену, остальные сворачиваются за кулисы.
    pub fn update(&mut self, windows: &[AppWindow], front: isize) -> Result<()> {
        let state: Vec<(AppWindow, bool)> = windows.iter().map(|w| (w.clone(), unsafe { IsIconic(HWND(w.hwnd as *mut _)) }.as_bool())).collect();
        let Some((minimize, side)) = plan(&state, front) else { return Ok(()) };
        for h in minimize {
            if let Some(shot) = snapshot(h) {
                self.shots.insert(h, shot);
            }
            unsafe {
                let _ = ShowWindow(HWND(h as *mut _), SW_MINIMIZE);
            }
        }
        self.shots.retain(|h, _| windows.iter().any(|w| w.hwnd == *h));
        if side != self.side {
            let px = (22.0 * self.scale).round() as i32;
            let tiles: Vec<StageTile> = side
                .iter()
                .map(|s| {
                    let icon = load_icon(&s.target, px);
                    let image = self.shots.get(&s.hwnd).map(|(rgba, w, h)| Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba, *w, *h)));
                    StageTile {
                        has_image: image.is_some(),
                        image: image.unwrap_or_default(),
                        has_icon: icon.is_some(),
                        icon: icon.as_ref().map(image_from_pixels).unwrap_or_default(),
                        title: s.title.as_str().into(),
                    }
                })
                .collect();
            self.view.ui.set_tiles(ModelRc::new(VecModel::from(tiles)));
            self.side = side;
        }
        self.present()
    }

    /// Окно, которое выбрали щелчком по полоске.
    pub fn tick(&mut self) -> Result<Option<isize>> {
        for ev in self.strip.take_input() {
            self.view.dispatch(&ev);
        }
        self.present_if_changed()?;
        let i = self.picked.replace(-1);
        Ok((i >= 0).then(|| self.side.get(i as usize).map(|s| s.hwnd)).flatten())
    }

    fn present_if_changed(&self) -> Result<()> {
        if let Some(content) = self.view.render_if_changed() {
            let r = self.strip.rect();
            let mut out = Bitmap::new(r.w as u32, r.h as u32);
            cpu::composite_over(&mut out, &content);
            self.strip.present(&out)?;
        }
        Ok(())
    }

    /// Stage Manager выключили: свёрнутые им окна возвращаются.
    pub fn restore_all(&mut self) {
        for s in self.side.drain(..) {
            unsafe {
                let h = HWND(s.hwnd as *mut _);
                if IsWindow(Some(h)).as_bool() && IsIconic(h).as_bool() {
                    let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                }
            }
        }
    }

    pub fn animating(&self) -> bool {
        self.view.animating()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(hwnd: isize, key: &str) -> AppWindow {
        AppWindow { hwnd, key: key.into(), target: format!("c:\\{key}.exe"), title: format!("t{hwnd}") }
    }

    #[test]
    fn the_front_app_stays_and_every_other_visible_window_is_minimized() {
        let list = vec![(w(1, "a"), false), (w(2, "b"), false), (w(3, "a"), false), (w(4, "c"), true)];
        let (minimize, side) = plan(&list, 1).unwrap();
        assert_eq!(minimize, vec![2], "a's second window stays, c is already minimized");
        assert_eq!(side.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), vec!["b", "c"]);
        assert_eq!(side[0].hwnd, 2);
    }

    #[test]
    fn an_unknown_front_window_changes_nothing() {
        assert!(plan(&[(w(1, "a"), false)], 99).is_none(), "the desktop or one of our own windows in front");
    }

    #[test]
    fn one_tile_per_app_and_the_strip_is_capped() {
        let mut list = vec![(w(1, "front"), false)];
        for i in 0..10 {
            list.push((w(10 + i, &format!("app{i}")), true));
            list.push((w(30 + i, &format!("app{i}")), true));
        }
        let (minimize, side) = plan(&list, 1).unwrap();
        assert!(minimize.is_empty());
        assert_eq!(side.len(), MAX_SIDE);
        assert_eq!(side[0].hwnd, 10, "the top window of an app represents it");
    }
}
