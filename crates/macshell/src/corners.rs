//! Горячие углы: курсор ушёл в угол экрана и постоял, и выполняется действие из конфига (`corner_tl`, `corner_tr`,
//! `corner_bl`, `corner_br`). В каждом углу стоит крошечное почти невидимое окно: мышь сама присылает ему события,
//! опрашивать курсор не нужно. Действие срабатывает один раз за заход в угол, повторно только после выхода из него.

use std::time::{Duration, Instant};

use anyhow::Result;
use capture::RectI;
use glass::Bitmap;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::host::{InputEvent, LayeredWindow};

/// Сторона окна в углу, физические пиксели.
const SIDE: i32 = 3;
/// Сколько курсор должен постоять в углу.
pub const DWELL: Duration = Duration::from_millis(300);

/// Угол по номеру из конфига (0 верхний левый, 1 верхний правый, 2 нижний левый, 3 нижний правый): место окна.
pub fn corner_rect(index: usize, screen_w: i32, screen_h: i32) -> RectI {
    let x = if index % 2 == 0 { 0 } else { screen_w - SIDE };
    let y = if index < 2 { 0 } else { screen_h - SIDE };
    RectI { x, y, w: SIDE, h: SIDE }
}

/// Нужно ли окно в этом углу: действие задано.
pub fn wants_window(action: &str) -> bool {
    !action.trim().is_empty()
}

fn corner_window(rect: RectI) -> Result<LayeredWindow> {
    let win = LayeredWindow::new(rect)?;
    let mut bmp = Bitmap::new(rect.w as u32, rect.h as u32);
    bmp.pixels.fill(0x0100_0000); // 1% непрозрачности: совсем прозрачные точки мышь пропускают
    win.present(&bmp)?;
    Ok(win)
}

pub struct Corners {
    wins: [Option<LayeredWindow>; 4],
    actions: [String; 4],
    entered: [Option<Instant>; 4],
    fired: [bool; 4],
}

impl Corners {
    /// Окна в углах с заданным действием; `None`, если не задано ни одно.
    pub fn open(actions: &[String; 4]) -> Option<Corners> {
        if !actions.iter().any(|a| wants_window(a)) {
            return None;
        }
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let mut wins: [Option<LayeredWindow>; 4] = [None, None, None, None];
        for (i, action) in actions.iter().enumerate() {
            if wants_window(action) {
                wins[i] = corner_window(corner_rect(i, sw, sh)).ok();
            }
        }
        Some(Corners { wins, actions: actions.clone(), entered: [None; 4], fired: [false; 4] })
    }

    /// События мыши и таймер стояния. Возвращает действия, которые пора выполнить.
    pub fn tick(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..4 {
            let Some(win) = &self.wins[i] else { continue };
            for ev in win.take_input() {
                match ev {
                    InputEvent::Move { .. } => {
                        self.entered[i].get_or_insert_with(Instant::now);
                    }
                    InputEvent::Leave => {
                        self.entered[i] = None;
                        self.fired[i] = false;
                    }
                    _ => {}
                }
            }
            if let Some(at) = self.entered[i] {
                if !self.fired[i] && at.elapsed() >= DWELL {
                    self.fired[i] = true;
                    out.push(self.actions[i].clone());
                }
            }
        }
        out
    }

    /// Через сколько миллисекунд проверить стояние в углу (иначе окну не нужны шаги цикла).
    pub fn next_wake_ms(&self) -> u32 {
        self.entered
            .iter()
            .zip(&self.fired)
            .filter_map(|(at, fired)| at.filter(|_| !fired))
            .map(|at| DWELL.saturating_sub(at.elapsed()).as_millis() as u32 + 1)
            .min()
            .unwrap_or(u32::MAX)
    }

    /// Поверх всех окон (`true`) или под всеми (полноэкранное приложение).
    pub fn set_topmost(&self, on: bool) {
        for w in self.wins.iter().flatten() {
            w.set_topmost(on);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_windows_sit_in_the_four_corners() {
        let r = |i| corner_rect(i, 1920, 1080);
        assert_eq!((r(0).x, r(0).y), (0, 0));
        assert_eq!((r(1).x, r(1).y), (1917, 0));
        assert_eq!((r(2).x, r(2).y), (0, 1077));
        assert_eq!((r(3).x, r(3).y), (1917, 1077));
        assert!((0..4).all(|i| r(i).w == 3 && r(i).h == 3));
    }

    #[test]
    fn only_corners_with_an_action_get_a_window() {
        assert!(wants_window("mission") && !wants_window("") && !wants_window("  "));
        assert!(Corners::open(&Default::default()).is_none(), "nothing configured: no windows at all");
    }
}
