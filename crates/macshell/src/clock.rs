//! Часы для верхней полосы: формат «Пн 2 нояб. 20:31» и время до границы минуты.

use windows::Win32::System::SystemInformation::GetLocalTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    /// 0 = воскресенье, как в `SYSTEMTIME`.
    pub weekday: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

impl LocalTime {
    pub fn now() -> LocalTime {
        let t = unsafe { GetLocalTime() };
        LocalTime {
            year: t.wYear,
            month: t.wMonth,
            day: t.wDay,
            weekday: t.wDayOfWeek,
            hour: t.wHour,
            minute: t.wMinute,
            second: t.wSecond,
            millis: t.wMilliseconds,
        }
    }
}

const WEEKDAYS: [&str; 7] = ["Вс", "Пн", "Вт", "Ср", "Чт", "Пт", "Сб"];
const MONTHS: [&str; 12] =
    ["янв.", "февр.", "мар.", "апр.", "мая", "июн.", "июл.", "авг.", "сент.", "окт.", "нояб.", "дек."];

/// Что показывают часы (`clock = "seconds,12h,nodate,noweekday"` в конфиге). Значения по умолчанию, все `false`,
/// дают «Пн 2 нояб. 20:31».
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockOptions {
    pub seconds: bool,
    pub hour12: bool,
    pub hide_date: bool,
    pub hide_weekday: bool,
}

impl ClockOptions {
    /// Разбирает список слов через запятую; незнакомые слова пропускаются.
    pub fn parse(text: &str) -> ClockOptions {
        let mut o = ClockOptions::default();
        for word in text.split(',').map(|w| w.trim().to_lowercase()) {
            match word.as_str() {
                "seconds" => o.seconds = true,
                "12h" => o.hour12 = true,
                "nodate" => o.hide_date = true,
                "noweekday" => o.hide_weekday = true,
                _ => {}
            }
        }
        o
    }

    /// Обратное `parse`; пустая строка, если всё по умолчанию.
    pub fn serialize(&self) -> String {
        let mut words = Vec::new();
        for (on, word) in [(self.seconds, "seconds"), (self.hour12, "12h"), (self.hide_date, "nodate"), (self.hide_weekday, "noweekday")] {
            if on {
                words.push(word);
            }
        }
        words.join(",")
    }
}

/// «Пн 2 нояб. 20:31»: час без нуля, минуты с нулём.
pub fn format_clock(t: &LocalTime) -> String {
    format_clock_with(t, &ClockOptions::default())
}

/// Часы по выбранным параметрам: день недели и дата по желанию, секунды, 12-часовой вид («8:31 PM»).
pub fn format_clock_with(t: &LocalTime, o: &ClockOptions) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !o.hide_weekday {
        parts.push(WEEKDAYS[(t.weekday % 7) as usize].to_string());
    }
    if !o.hide_date {
        parts.push(format!("{} {}", t.day, MONTHS[(t.month.clamp(1, 12) - 1) as usize]));
    }
    let (hour, suffix) = if o.hour12 { (if t.hour % 12 == 0 { 12 } else { t.hour % 12 }, if t.hour < 12 { " AM" } else { " PM" }) } else { (t.hour, "") };
    let seconds = if o.seconds { format!(":{:02}", t.second) } else { String::new() };
    parts.push(format!("{hour}:{:02}{seconds}{suffix}", t.minute));
    parts.join(" ")
}

/// Сколько миллисекунд до следующей границы минуты (на границе: целая минута).
pub fn ms_until_next_minute(t: &LocalTime) -> u32 {
    60_000 - (t.second as u32 * 1000 + t.millis as u32)
}

/// До следующей перерисовки часов: граница секунды, если секунды показываются, иначе граница минуты.
pub fn ms_until_next_tick(t: &LocalTime, o: &ClockOptions) -> u32 {
    if o.seconds { 1000 - t.millis as u32 } else { ms_until_next_minute(t) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(month: u16, day: u16, weekday: u16, hour: u16, minute: u16, second: u16, millis: u16) -> LocalTime {
        LocalTime { year: 2026, month, day, weekday, hour, minute, second, millis }
    }

    #[test]
    fn formats_monday_november() {
        assert_eq!(format_clock(&at(11, 2, 1, 20, 31, 0, 0)), "Пн 2 нояб. 20:31");
    }

    #[test]
    fn pads_minutes() {
        assert_eq!(format_clock(&at(11, 2, 1, 9, 5, 0, 0)), "Пн 2 нояб. 9:05");
    }

    #[test]
    fn months_have_russian_abbreviations() {
        let expected = ["янв.", "февр.", "мар.", "апр.", "мая", "июн.", "июл.", "авг.", "сент.", "окт.", "нояб.", "дек."];
        for (i, abbr) in expected.iter().enumerate() {
            let text = format_clock(&at(i as u16 + 1, 15, 3, 12, 0, 0, 0));
            assert!(text.contains(&format!(" {abbr} ")), "{text}");
        }
    }

    #[test]
    fn options_change_what_is_shown() {
        let t = at(11, 2, 1, 20, 31, 7, 0);
        let show = |words: &str| format_clock_with(&t, &ClockOptions::parse(words));
        assert_eq!(show(""), "Пн 2 нояб. 20:31");
        assert_eq!(show("seconds"), "Пн 2 нояб. 20:31:07");
        assert_eq!(show("12h"), "Пн 2 нояб. 8:31 PM");
        assert_eq!(show("nodate"), "Пн 20:31");
        assert_eq!(show("noweekday"), "2 нояб. 20:31");
        assert_eq!(show("nodate,noweekday,seconds"), "20:31:07");
        let midnight = at(11, 2, 1, 0, 5, 0, 0);
        assert_eq!(format_clock_with(&midnight, &ClockOptions { hour12: true, ..Default::default() }), "Пн 2 нояб. 12:05 AM");
    }

    #[test]
    fn options_roundtrip_and_ignore_unknown_words() {
        let o = ClockOptions::parse(" Seconds , 12H, bogus");
        assert_eq!(o, ClockOptions { seconds: true, hour12: true, ..Default::default() });
        assert_eq!(ClockOptions::parse(&o.serialize()), o);
        assert_eq!(ClockOptions::default().serialize(), "");
    }

    #[test]
    fn redraw_waits_for_the_second_only_when_seconds_show() {
        let t = at(11, 2, 1, 20, 31, 30, 250);
        assert_eq!(ms_until_next_tick(&t, &ClockOptions::default()), 29_750);
        assert_eq!(ms_until_next_tick(&t, &ClockOptions { seconds: true, ..Default::default() }), 750);
    }

    #[test]
    fn next_minute_mid() {
        assert_eq!(ms_until_next_minute(&at(11, 2, 1, 20, 31, 30, 500)), 29_500);
    }

    #[test]
    fn next_minute_edge() {
        assert_eq!(ms_until_next_minute(&at(11, 2, 1, 20, 31, 59, 999)), 1);
    }

    #[test]
    fn next_minute_exact() {
        assert_eq!(ms_until_next_minute(&at(11, 2, 1, 20, 31, 0, 0)), 60_000);
    }
}
