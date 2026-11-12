//! Данные для виджетов рабочего стола, которым нужна сеть или система: погода (Open-Meteo, место по IP или из
//! конфига) и диски. Сетевые запросы идут в фоновом потоке, виджет получает ответ по каналу и не ждёт.

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
use windows::core::PCWSTR;

const TIMEOUT_MS: i32 = 8000;
const DRIVE_FIXED: u32 = 3;

#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub city: String,
    pub temp: i32,
    pub feels: i32,
    pub code: u32,
    pub max: i32,
    pub min: i32,
}

/// Описание погоды по коду WMO и номер значка: 0 солнце, 1 солнце за облаком, 2 облако, 3 туман, 4 дождь, 5 снег,
/// 6 гроза.
pub fn wmo(code: u32) -> (&'static str, i32) {
    match code {
        0 => ("Ясно", 0),
        1 => ("Малооблачно", 1),
        2 => ("Переменная облачность", 1),
        3 => ("Пасмурно", 2),
        45 | 48 => ("Туман", 3),
        51..=57 => ("Морось", 4),
        61..=65 | 80..=82 => ("Дождь", 4),
        66 | 67 => ("Ледяной дождь", 4),
        71..=77 | 85 | 86 => ("Снег", 5),
        95..=99 => ("Гроза", 6),
        _ => ("Погода", 2),
    }
}

/// Число сразу после `key` (например `"temperature_2m":` или `"temperature_2m_max":[`).
fn number_after(body: &str, key: &str) -> Option<f64> {
    let start = body.find(key)? + key.len();
    let rest = body[start..].trim_start();
    let end = rest.find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Строка в кавычках сразу после `key` (например `"city":`).
fn string_after(body: &str, key: &str) -> Option<String> {
    let start = body.find(key)? + key.len();
    let rest = body[start..].trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Ответ Open-Meteo (`current` и `daily` на сегодня) в погоду для `city`.
pub fn parse_weather(body: &str, city: &str) -> Option<Weather> {
    let cur = &body[body.find("\"current\":{")?..];
    let day = &body[body.find("\"daily\":{")?..];
    let round = |v: f64| v.round() as i32;
    Some(Weather {
        city: city.to_string(),
        temp: round(number_after(cur, "\"temperature_2m\":")?),
        feels: round(number_after(cur, "\"apparent_temperature\":")?),
        code: number_after(cur, "\"weather_code\":")? as u32,
        max: round(number_after(day, "\"temperature_2m_max\":[")?),
        min: round(number_after(day, "\"temperature_2m_min\":[")?),
    })
}

/// Ответ определения места по IP (`ipwho.is`): широта, долгота, город.
pub fn parse_location(body: &str) -> Option<(f64, f64, String)> {
    if body.contains("\"success\":false") {
        return None;
    }
    let city = string_after(body, "\"city\":").filter(|c| !c.is_empty()).unwrap_or_else(|| "Погода".into());
    Some((number_after(body, "\"latitude\":")?, number_after(body, "\"longitude\":")?, city))
}

/// Место из конфига: `55.75,37.62,Москва`.
pub fn parse_place(text: &str) -> Option<(f64, f64, String)> {
    let mut parts = text.splitn(3, ',');
    let lat: f64 = parts.next()?.trim().parse().ok()?;
    let lon: f64 = parts.next()?.trim().parse().ok()?;
    let name = parts.next().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Погода");
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then(|| (lat, lon, name.to_string()))
}

fn forecast_url(lat: f64, lon: f64) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat:.3}&longitude={lon:.3}&current=temperature_2m,apparent_temperature,weather_code&daily=temperature_2m_max,temperature_2m_min&forecast_days=1&timezone=auto"
    )
}

/// Найденное по IP место помнится до перезапуска: оно не меняется, а запрос к чужому сервису лишний.
static PLACE: OnceLock<Mutex<Option<(f64, f64, String)>>> = OnceLock::new();

/// Запрашивает погоду; ответ приходит в канал (`None` при любой ошибке сети). `configured`: место из конфига
/// (`weather = "55.75,37.62,Москва"`), иначе оно определяется по IP.
pub fn request_weather(configured: &str) -> Receiver<Option<Weather>> {
    let configured = parse_place(configured);
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new().name("weather".into()).spawn(move || {
        let place = configured.or_else(|| {
            let cache = PLACE.get_or_init(|| Mutex::new(None));
            if let Some(p) = cache.lock().ok().and_then(|g| g.clone()) {
                return Some(p);
            }
            let body = crate::http::get("https://ipwho.is/?lang=ru&fields=success,city,latitude,longitude", TIMEOUT_MS)?;
            let place = parse_location(&String::from_utf8_lossy(&body))?;
            if let Ok(mut g) = cache.lock() {
                *g = Some(place.clone());
            }
            Some(place)
        });
        let weather = place.and_then(|(lat, lon, city)| {
            let body = crate::http::get(&forecast_url(lat, lon), TIMEOUT_MS)?;
            parse_weather(&String::from_utf8_lossy(&body), &city)
        });
        let _ = tx.send(weather);
    });
    rx
}

/// Постоянный диск: имя («C:»), свободно и всего байт.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    pub name: String,
    pub free: u64,
    pub total: u64,
}

/// Постоянные диски компьютера; съёмные и сетевые не берутся.
pub fn drives() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(Some(0)).collect();
        if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
            continue;
        }
        let (mut free, mut total) = (0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut free), Some(&mut total), None) }.is_ok() && total > 0 {
            out.push(Drive { name: format!("{letter}:"), free, total });
        }
    }
    out
}

/// «412 ГБ» или «1,8 ТБ».
pub fn size_text(bytes: u64) -> String {
    let gb = bytes as f64 / (1u64 << 30) as f64;
    if gb >= 1000.0 { format!("{:.1} ТБ", gb / 1024.0).replace('.', ",") } else { format!("{} ГБ", gb.round() as u64) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORECAST: &str = r#"{"latitude":55.75,"current_units":{"time":"iso8601","temperature_2m":"°C","apparent_temperature":"°C","weather_code":"wmo code"},"current":{"time":"2026-11-03T13:00","interval":900,"temperature_2m":4.6,"apparent_temperature":1.2,"weather_code":61},"daily_units":{"temperature_2m_max":"°C"},"daily":{"time":["2026-11-03"],"temperature_2m_max":[6.4],"temperature_2m_min":[-1.5]}}"#;

    #[test]
    fn forecast_json_gives_temperatures_and_code_from_the_right_sections() {
        let w = parse_weather(FORECAST, "Москва").unwrap();
        assert_eq!(w, Weather { city: "Москва".into(), temp: 5, feels: 1, code: 61, max: 6, min: -2 });
        assert!(parse_weather("{}", "x").is_none());
        assert!(parse_weather(r#"{"current":{"temperature_2m":1}}"#, "x").is_none(), "needs the daily part too");
    }

    #[test]
    fn location_comes_from_the_ip_reply_or_the_config() {
        let reply = r#"{"success":true,"city":"Москва","latitude":55.7558,"longitude":37.6173}"#;
        assert_eq!(parse_location(reply), Some((55.7558, 37.6173, "Москва".into())));
        assert!(parse_location(r#"{"success":false,"message":"x"}"#).is_none());
        assert_eq!(parse_place("55.75, 37.62, Санкт-Петербург, Россия"), Some((55.75, 37.62, "Санкт-Петербург, Россия".into())));
        assert_eq!(parse_place("55.75,37.62"), Some((55.75, 37.62, "Погода".into())));
        assert!(parse_place("95,10,x").is_none() && parse_place("junk").is_none() && parse_place("").is_none());
    }

    #[test]
    fn weather_codes_map_to_words_and_icons() {
        assert_eq!(wmo(0), ("Ясно", 0));
        assert_eq!(wmo(3), ("Пасмурно", 2));
        assert_eq!(wmo(63).1, 4);
        assert_eq!(wmo(73).1, 5);
        assert_eq!(wmo(95).1, 6);
        assert_eq!(wmo(999).1, 2, "unknown codes get the plain cloud");
    }

    #[test]
    fn sizes_switch_to_terabytes() {
        assert_eq!(size_text(412 << 30), "412 ГБ");
        assert_eq!(size_text(1_900_000_000_000), "1,7 ТБ");
    }

    #[test]
    fn this_computer_has_a_fixed_drive() {
        let d = drives();
        assert!(d.iter().all(|x| x.free <= x.total && x.name.len() == 2));
        assert!(!d.is_empty());
    }
}
