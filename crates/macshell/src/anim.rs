//! Кривые и время для анимаций. Окна рисуют кадры сами: пока что-то движется, цикл просыпается раз в 16 мс,
//! в покое никаких таймеров нет.

use std::time::Instant;

/// Доля пути от 0 до 1 за `duration_ms` с момента `start`.
pub fn progress(start: Instant, duration_ms: u32) -> f32 {
    (start.elapsed().as_secs_f32() * 1000.0 / duration_ms as f32).clamp(0.0, 1.0)
}

/// Быстрый старт, мягкое замедление: появление.
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Медленный старт, быстрый конец: исчезновение.
pub fn ease_in_cubic(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(3)
}

/// Плавный переход прозрачности и смещения; можно начать новый с текущего кадра.
#[derive(Clone, Copy, Debug)]
pub struct WindowTween {
    from: (f32, f32),
    to: (f32, f32),
    duration_ms: u32,
}

impl WindowTween {
    pub fn new(from: (f32, f32), to: (f32, f32), duration_ms: u32) -> Self {
        Self { from, to, duration_ms }
    }

    pub fn finished(&self, elapsed_ms: u32) -> bool {
        elapsed_ms >= self.duration_ms
    }

    pub fn sample(&self, elapsed_ms: u32) -> (f32, f32) {
        let t = if self.duration_ms == 0 { 1.0 } else { ease_out_cubic(elapsed_ms as f32 / self.duration_ms as f32) };
        (self.from.0 + (self.to.0 - self.from.0) * t, self.from.1 + (self.to.1 - self.from.1) * t)
    }
}

/// Уважает системный переключатель анимаций; если Windows не ответила, поведение остаётся обычным.
pub fn enabled() -> bool {
    use windows::core::BOOL;
    use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
    let mut enabled = BOOL(1);
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut enabled as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) }
        .is_err() || enabled.as_bool()
}

/// Сколько кадров занимает анимация: шаг цикла 16 мс.
pub const FRAME_MS: u32 = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_window_motion_continues_from_its_current_frame() {
        let open = WindowTween::new((0.0, -8.0), (1.0, 0.0), 200);
        let halfway = open.sample(80);
        assert!(halfway.0 > 0.0 && halfway.0 < 1.0);
        let close = WindowTween::new(halfway, (0.0, -4.0), 130);
        assert_eq!(close.sample(0), halfway);
        assert_eq!(close.sample(130), (0.0, -4.0));
        assert_eq!(close.sample(500), (0.0, -4.0));
    }

    #[test]
    fn zero_duration_moves_directly_to_the_end() {
        let transition = WindowTween::new((0.0, 8.0), (1.0, 0.0), 0);
        assert_eq!(transition.sample(0), (1.0, 0.0));
        assert!(transition.finished(0));
    }

    #[test]
    fn easing_fixes_endpoints_and_clamps() {
        for f in [ease_out_cubic, ease_in_cubic] {
            assert_eq!(f(0.0), 0.0);
            assert_eq!(f(1.0), 1.0);
            assert_eq!(f(-3.0), 0.0);
            assert_eq!(f(9.0), 1.0);
        }
    }

    #[test]
    fn out_is_fast_first_in_is_slow_first() {
        assert!(ease_out_cubic(0.25) > 0.25);
        assert!(ease_in_cubic(0.25) < 0.25);
    }

    #[test]
    fn easing_is_monotonic() {
        let mut prev = (0.0, 0.0);
        for i in 1..=20 {
            let t = i as f32 / 20.0;
            let cur = (ease_out_cubic(t), ease_in_cubic(t));
            assert!(cur.0 >= prev.0 && cur.1 >= prev.1);
            prev = cur;
        }
    }

    #[test]
    fn progress_starts_near_zero_and_never_exceeds_one() {
        let now = Instant::now();
        assert!(progress(now, 10_000) < 0.01);
        let long_ago = Instant::now() - std::time::Duration::from_secs(5);
        assert_eq!(progress(long_ago, 100), 1.0);
    }
}
