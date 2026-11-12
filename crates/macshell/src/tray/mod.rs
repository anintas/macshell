//! Трей: хост окна `Shell_TrayWnd` для значков приложений (`Shell_NotifyIcon`).

pub mod fallback;
pub mod host;
pub mod proto;
pub mod state;

/// Размер окна значков трея в логических пикселях для `n` значков: сетка по 4, плитка 44, шаг 48,
/// внизу две строки подписи (название и подсказка). Совпадает с `ui/trayflyout.slint`.
pub fn flyout_size(n: usize) -> (f32, f32) {
    let rows = n.div_ceil(4).max(1) as f32;
    (204.0, 46.0 + 48.0 * rows)
}

/// Центр плитки номер `i` в логических пикселях окна значков.
pub fn tile_center(i: usize) -> (f32, f32) {
    (8.0 + 22.0 + (i % 4) as f32 * 48.0, 8.0 + 22.0 + (i / 4) as f32 * 48.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flyout_grows_by_rows_of_four() {
        assert_eq!(flyout_size(0), (204.0, 94.0));
        assert_eq!(flyout_size(4), (204.0, 94.0));
        assert_eq!(flyout_size(5), (204.0, 142.0));
        assert_eq!(flyout_size(8), (204.0, 142.0));
        assert_eq!(flyout_size(9), (204.0, 190.0));
    }

    #[test]
    fn tile_centers_follow_the_grid() {
        assert_eq!(tile_center(0), (30.0, 30.0));
        assert_eq!(tile_center(3), (174.0, 30.0));
        assert_eq!(tile_center(4), (30.0, 78.0));
    }
}

/// Первая строка подсказки значка: у многих приложений в подсказке несколько строк.
pub fn first_line(tip: &str) -> String {
    tip.lines().next().unwrap_or("").trim().chars().take(80).collect()
}

#[cfg(test)]
mod tip_tests {
    use super::*;

    #[test]
    fn first_line_takes_only_the_first_and_trims() {
        assert_eq!(first_line("  Загрузка: 0 Б/с \nОтдача: 0 Б/с"), "Загрузка: 0 Б/с");
        assert_eq!(first_line(""), "");
        assert_eq!(first_line(&"я".repeat(200)).chars().count(), 80);
    }
}
