//! Календарь для окна под часами: сетка месяца (неделя с понедельника), заголовок, переход между месяцами.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Day {
    pub day: u32,
    /// `false` для хвоста прошлого и начала следующего месяца: они только заполняют сетку.
    pub in_month: bool,
    pub today: bool,
}

pub const WEEKDAYS: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
const MONTHS: [&str; 12] = [
    "январь", "февраль", "март", "апрель", "май", "июнь", "июль", "август", "сентябрь", "октябрь", "ноябрь", "декабрь",
];

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

/// Дни от 1970-01-01 (алгоритм Говарда Хиннанта).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// День недели, 0 = понедельник … 6 = воскресенье.
pub fn weekday(year: i32, month: u32, day: u32) -> u32 {
    // 1970-01-01 был четвергом.
    (days_from_civil(year, month, day) + 3).rem_euclid(7) as u32
}

/// Месяц со сдвигом на `delta` месяцев; год переходит сам.
pub fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let index = year * 12 + (month as i32 - 1) + delta;
    (index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
}

/// «ноябрь 2026».
pub fn title(year: i32, month: u32) -> String {
    format!("{} {year}", MONTHS[(month.clamp(1, 12) - 1) as usize])
}

/// Шесть недель по семь дней: месяц целиком, остальное добирается соседними месяцами.
pub fn month_grid(year: i32, month: u32, today: (i32, u32, u32)) -> Vec<Day> {
    let lead = weekday(year, month, 1);
    let (prev_year, prev_month) = shift_month(year, month, -1);
    let prev_days = days_in_month(prev_year, prev_month);
    let this_days = days_in_month(year, month);
    (0..42u32)
        .map(|i| {
            let (y, m, d, in_month) = if i < lead {
                (prev_year, prev_month, prev_days - lead + i + 1, false)
            } else if i < lead + this_days {
                (year, month, i - lead + 1, true)
            } else {
                let (ny, nm) = shift_month(year, month, 1);
                (ny, nm, i - lead - this_days + 1, false)
            };
            Day { day: d, in_month, today: (y, m, d) == today }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_weekdays() {
        assert_eq!(weekday(2026, 11, 2), 0, "2 ноября 2026 понедельник");
        assert_eq!(weekday(2026, 11, 1), 6);
        assert_eq!(weekday(1970, 1, 1), 3, "четверг");
        assert_eq!(weekday(2000, 2, 29), 1, "вторник");
    }

    #[test]
    fn month_lengths_and_leap_years() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2026, 11), 30);
        assert_eq!(days_in_month(2026, 12), 31);
    }

    #[test]
    fn shift_crosses_years_both_ways() {
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 11, 0), (2026, 11));
        assert_eq!(shift_month(2026, 11, 14), (2028, 1));
        assert_eq!(shift_month(2026, 3, -15), (2024, 12));
    }

    #[test]
    fn title_uses_nominative_month() {
        assert_eq!(title(2026, 11), "ноябрь 2026");
        assert_eq!(title(2027, 1), "январь 2027");
    }

    #[test]
    fn november_2026_grid() {
        let g = month_grid(2026, 11, (2026, 11, 2));
        assert_eq!(g.len(), 42);
        // Ноябрь начинается с воскресенья: шесть дней октября (26..31) впереди.
        assert_eq!(g[0], Day { day: 26, in_month: false, today: false });
        assert_eq!(g[5], Day { day: 31, in_month: false, today: false });
        assert_eq!(g[6], Day { day: 1, in_month: true, today: false });
        assert_eq!(g[7], Day { day: 2, in_month: true, today: true });
        assert_eq!(g[35], Day { day: 30, in_month: true, today: false });
        assert_eq!(g[36], Day { day: 1, in_month: false, today: false });
        assert_eq!(g[41], Day { day: 6, in_month: false, today: false });
        assert_eq!(g.iter().filter(|d| d.today).count(), 1);
    }

    #[test]
    fn month_starting_on_monday_has_no_leading_days() {
        // Июнь 2026 начинается с понедельника.
        let g = month_grid(2026, 6, (2000, 1, 1));
        assert!(g[0].in_month && g[0].day == 1);
        assert!(g.iter().all(|d| !d.today));
    }

    #[test]
    fn today_in_a_neighbour_month_is_marked_on_the_filler_cell() {
        // 31 октября видно в сетке ноября.
        let g = month_grid(2026, 11, (2026, 10, 31));
        assert!(g[5].today && !g[5].in_month);
    }

    #[test]
    fn february_leap_year_grid_contains_29() {
        let g = month_grid(2024, 2, (2024, 2, 29));
        assert_eq!(g.iter().filter(|d| d.in_month).count(), 29);
        assert!(g.iter().any(|d| d.in_month && d.day == 29 && d.today));
    }
}
