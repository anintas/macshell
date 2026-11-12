//! Конвертация валют для Spotlight: `100 usd in eur`, `5000 руб в доллары`, `10 € to ₽`. Курсы берутся с сайта
//! Банка России (`cbr.ru`, XML на текущий день, около 55 валют к рублю), кросс-курсы считаются через рубль.
//! Запрос в сеть уходит только когда набран запрос про валюты и курсы старше 12 часов, не чаще раза в 10 минут;
//! курсы лежат в памяти и в `rates.xml` рядом с конфигом (чтобы после перезапуска сеть не требовалась).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::units;

pub const URL: &str = "https://www.cbr.ru/scripts/XML_daily.asp";
const MAX_AGE: Duration = Duration::from_secs(12 * 3600);
const RETRY_AFTER: Duration = Duration::from_secs(600);
const TIMEOUT_MS: i32 = 8000;

/// Коды валют, которые публикует ЦБ (и рубль): ими можно писать без слов.
const ISO_CODES: &[&str] = &[
    "AUD", "AZN", "GBP", "AMD", "BYN", "BGN", "BRL", "HUF", "VND", "HKD", "GEL", "DKK", "AED", "USD", "EUR", "EGP", "INR", "IDR",
    "KZT", "CAD", "QAR", "KGS", "CNY", "MDL", "NZD", "NOK", "PLN", "RON", "RUB", "XDR", "SGD", "TJS", "THB", "TRY", "TMT", "UZS",
    "UAH", "CZK", "SEK", "CHF", "RSD", "ZAR", "KRW", "JPY",
];

/// Слова и знаки валют. Слова с общими значениями (`фунт`, `рубль`) подобраны так, чтобы не пересекаться с
/// единицами из `units`: `фунт` без «стерлингов» остаётся мерой массы.
const ALIASES: &[(&str, &[&str])] = &[
    ("USD", &["$", "dollar", "dollars", "доллар", "доллара", "долларов", "долл", "бакс", "баксов", "баксы"]),
    ("EUR", &["€", "euro", "euros", "евро"]),
    ("RUB", &["₽", "ruble", "rubles", "руб", "рубль", "рубля", "рублей", "р"]),
    ("GBP", &["£", "pound sterling", "pounds sterling", "фунт стерлингов", "фунта стерлингов", "фунтов стерлингов", "стерлингов"]),
    ("CNY", &["yuan", "rmb", "юань", "юаня", "юаней"]),
    ("JPY", &["yen", "иена", "иены", "иен", "йена", "йены", "йен"]),
    ("KZT", &["tenge", "тенге"]),
    ("UAH", &["hryvnia", "гривна", "гривны", "гривен"]),
    ("TRY", &["lira", "лира", "лиры", "лир"]),
    ("CHF", &["swiss franc", "франк", "франка", "франков"]),
    ("BYN", &["белорусский рубль", "белорусских рублей", "белорусских рубля"]),
    ("INR", &["rupee", "rupees", "рупия", "рупии", "рупий"]),
    ("KRW", &["won", "вона", "вон"]),
    ("PLN", &["zloty", "злотый", "злотых", "злотого"]),
    ("GEL", &["lari", "лари"]),
    ("AMD", &["dram", "драм", "драмов"]),
];

/// Курсы на одну дату: сколько рублей стоит одна единица валюты.
#[derive(Clone, Debug, PartialEq)]
pub struct Rates {
    /// Дата курсов «дд.мм.гггг».
    pub date: String,
    rub: HashMap<String, f64>,
}

/// Содержимое тега `<name>…</name>` внутри `segment`.
fn tag<'a>(segment: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = segment.find(&open)? + open.len();
    let end = segment[start..].find(&format!("</{name}>"))? + start;
    Some(segment[start..end].trim())
}

/// Разбирает XML ЦБ (кодировка windows-1251: русские названия нам не нужны, остальное чистый ASCII).
pub fn parse(xml: &[u8]) -> Option<Rates> {
    let text = String::from_utf8_lossy(xml);
    let date = {
        let at = text.find("Date=\"")? + 6;
        text[at..at + text[at..].find('"')?].to_string()
    };
    let mut rub = HashMap::new();
    rub.insert("RUB".to_string(), 1.0);
    for segment in text.split("<Valute ").skip(1) {
        let (Some(code), Some(nominal), Some(value)) = (tag(segment, "CharCode"), tag(segment, "Nominal"), tag(segment, "Value")) else {
            continue;
        };
        let (Ok(nominal), Ok(value)) = (nominal.replace(',', ".").parse::<f64>(), value.replace(',', ".").parse::<f64>()) else {
            continue;
        };
        if nominal > 0.0 && value > 0.0 {
            rub.insert(code.to_uppercase(), value / nominal);
        }
    }
    // Ответ без основных валют считается испорченным: лучше прошлые курсы, чем пустые.
    (rub.contains_key("USD") && rub.contains_key("EUR") && rub.len() > 10).then_some(Rates { date, rub })
}

impl Rates {
    pub fn has(&self, code: &str) -> bool {
        self.rub.contains_key(code)
    }

    /// `value` единиц `from` в единицах `to`.
    pub fn convert(&self, value: f64, from: &str, to: &str) -> Option<f64> {
        Some(value * self.rub.get(from)? / self.rub.get(to)?)
    }
}

/// Код валюты по слову, знаку или самому коду («руб», «$», «usd»).
pub fn code_of(name: &str) -> Option<&'static str> {
    let n = name.trim().trim_end_matches('.').to_lowercase();
    if n.is_empty() {
        return None;
    }
    for (code, names) in ALIASES {
        if names.contains(&n.as_str()) {
            return Some(code);
        }
    }
    let upper = n.to_uppercase();
    ISO_CODES.iter().copied().find(|c| *c == upper)
}

/// Сумма: две цифры после запятой от единицы, меньше единицы четыре значащих.
fn format_money(v: f64) -> String {
    if v.abs() >= 1.0 {
        format!("{v:.2}").replace('.', ",")
    } else {
        units::format_value((v * 1e4).round() / 1e4)
    }
}

/// Дата курса для подписи: «03.11» из «03.11.2026».
fn short_date(date: &str) -> &str {
    date.rsplit_once('.').map_or(date, |(d, _)| d)
}

/// Чистая часть: перевод по готовым курсам. `(результат, дата курса)`.
pub fn convert_with(q: &units::ConversionQuery, rates: &Rates) -> Option<(String, String)> {
    let (from, to) = (code_of(&q.from)?, code_of(&q.to)?);
    let result = rates.convert(q.value, from, to)?;
    Some((format!("{} {to}", format_money(result)), short_date(&rates.date).to_string()))
}

/// Что вышло из запроса про валюты.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Не запрос про валюты.
    NotCurrency,
    /// Результат и дата курса.
    Ready { text: String, date: String },
    /// Курсов ещё нет, идёт загрузка.
    Loading,
    /// Курсов нет и загрузить не вышло.
    Unavailable,
}

static RATES: Mutex<Option<Arc<Rates>>> = Mutex::new(None);
/// Когда лежащие в памяти курсы получены (время файла или загрузки).
static LOADED_AT: Mutex<Option<SystemTime>> = Mutex::new(None);
static FETCHING: AtomicBool = AtomicBool::new(false);
static UPDATED: AtomicBool = AtomicBool::new(false);
static FAILED_AT: Mutex<Option<Instant>> = Mutex::new(None);

fn cache_path() -> Option<std::path::PathBuf> {
    crate::config::Config::path().map(|p| p.with_file_name("rates.xml"))
}

fn load_cache() -> Option<(Rates, SystemTime)> {
    let path = cache_path()?;
    let rates = parse(&std::fs::read(&path).ok()?)?;
    Some((rates, std::fs::metadata(&path).ok()?.modified().ok()?))
}

fn is_stale(loaded: Option<SystemTime>) -> bool {
    loaded.is_none_or(|t| SystemTime::now().duration_since(t).map_or(false, |age| age > MAX_AGE))
}

/// Готовит курсы: берёт файл, если в памяти пусто, и запускает загрузку, если курсы старше 12 часов.
fn ensure() {
    if RATES.lock().map_or(true, |g| g.is_none()) {
        if let Some((rates, at)) = load_cache() {
            if let (Ok(mut r), Ok(mut t)) = (RATES.lock(), LOADED_AT.lock()) {
                *r = Some(Arc::new(rates));
                *t = Some(at);
            }
        }
    }
    let stale = is_stale(LOADED_AT.lock().ok().and_then(|g| *g));
    let recently_failed = FAILED_AT.lock().ok().and_then(|g| *g).is_some_and(|at| at.elapsed() < RETRY_AFTER);
    if stale && !recently_failed && !FETCHING.swap(true, Ordering::SeqCst) {
        let _ = std::thread::Builder::new().name("rates".into()).spawn(|| {
            // Две попытки: первое обращение к сайту иногда не успевает (DNS, соединение).
            let fetched = (0..2).find_map(|_| crate::http::get(URL, TIMEOUT_MS).and_then(|body| parse(&body).map(|r| (r, body))));
            match fetched {
                Some((rates, body)) => {
                    if let Some(path) = cache_path() {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(path, body);
                    }
                    if let (Ok(mut r), Ok(mut t)) = (RATES.lock(), LOADED_AT.lock()) {
                        *r = Some(Arc::new(rates));
                        *t = Some(SystemTime::now());
                    }
                    if let Ok(mut f) = FAILED_AT.lock() {
                        *f = None;
                    }
                    UPDATED.store(true, Ordering::SeqCst);
                }
                None => {
                    if let Ok(mut f) = FAILED_AT.lock() {
                        *f = Some(Instant::now());
                    }
                    UPDATED.store(true, Ordering::SeqCst); // окну пора сменить «загружаю» на «недоступно»
                }
            }
            FETCHING.store(false, Ordering::SeqCst);
        });
    }
}

/// Курсы или ответ на запрос про валюты по текущему состоянию. Первый такой запрос запускает загрузку.
pub fn convert(query: &str) -> Outcome {
    let Some(q) = units::parse_query(query) else { return Outcome::NotCurrency };
    let (Some(_), Some(_)) = (code_of(&q.from), code_of(&q.to)) else { return Outcome::NotCurrency };
    ensure();
    let rates = RATES.lock().ok().and_then(|g| g.clone());
    match rates {
        Some(r) => match convert_with(&q, &r) {
            Some((text, date)) => Outcome::Ready { text, date },
            None => Outcome::NotCurrency, // валюты нет в списке ЦБ
        },
        None if FAILED_AT.lock().ok().and_then(|g| *g).is_some() => Outcome::Unavailable,
        None => Outcome::Loading,
    }
}

/// Курсы для виджета: дата и «рублей за единицу» по основным валютам. Запускает загрузку, если курсов нет или они
/// старые; пока их нет, `None`.
pub fn widget_rates() -> Option<(String, Vec<(&'static str, f64)>)> {
    ensure();
    let rates = RATES.lock().ok().and_then(|g| g.clone())?;
    let rows = ["USD", "EUR", "CNY"].into_iter().filter_map(|c| rates.convert(1.0, c, "RUB").map(|v| (c, v))).collect();
    Some((rates.date.clone(), rows))
}

/// Загрузка закончилась (удачно или нет) с прошлого вопроса: список стоит перестроить. Забирается один раз.
pub fn take_updated() -> bool {
    UPDATED.swap(false, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Урезанный ответ ЦБ: названия в windows-1251 (здесь просто байты не ASCII), числа с запятой.
    fn sample() -> Vec<u8> {
        let mut xml = b"<?xml version=\"1.0\" encoding=\"windows-1251\"?><ValCurs Date=\"03.11.2026\" name=\"Foreign Currency Market\">".to_vec();
        let rows: &[(&str, u32, &str)] = &[
            ("USD", 1, "80,0000"),
            ("EUR", 1, "92,0000"),
            ("CNY", 1, "11,2500"),
            ("JPY", 100, "52,0000"),
            ("KZT", 100, "16,0000"),
            ("GBP", 1, "105,0000"),
            ("TRY", 10, "20,0000"),
            ("UAH", 10, "19,0000"),
            ("CHF", 1, "99,0000"),
            ("PLN", 1, "21,0000"),
            ("INR", 100, "95,0000"),
        ];
        for (code, nominal, value) in rows {
            xml.extend_from_slice(format!("<Valute ID=\"R0\"><NumCode>840</NumCode><CharCode>{code}</CharCode><Nominal>{nominal}</Nominal><Name>").as_bytes());
            xml.extend_from_slice(&[0xC4, 0xEE, 0xEB, 0xEB, 0xE0, 0xF0]); // «Доллар» в windows-1251
            xml.extend_from_slice(format!("</Name><Value>{value}</Value></Valute>").as_bytes());
        }
        xml.extend_from_slice(b"</ValCurs>");
        xml
    }

    fn rates() -> Rates {
        parse(&sample()).expect("sample parses")
    }

    fn q(text: &str) -> units::ConversionQuery {
        units::parse_query(text).unwrap()
    }

    #[test]
    fn parse_reads_date_nominal_and_comma_decimals() {
        let r = rates();
        assert_eq!(r.date, "03.11.2026");
        assert_eq!(r.rub["USD"], 80.0);
        assert_eq!(r.rub["JPY"], 0.52, "100 yen cost 52 rubles");
        assert_eq!(r.rub["RUB"], 1.0);
        assert!(r.has("KZT") && !r.has("AUD"));
    }

    #[test]
    fn broken_or_partial_answers_are_rejected() {
        assert!(parse(b"").is_none());
        assert!(parse(b"<html>blocked</html>").is_none());
        assert!(parse(b"<ValCurs Date=\"03.11.2026\"><Valute ID=\"x\"><CharCode>USD</CharCode><Nominal>1</Nominal><Value>80,0</Value></Valute></ValCurs>").is_none());
    }

    #[test]
    fn rouble_cross_rates_follow_from_the_table() {
        let r = rates();
        assert_eq!(convert_with(&q("100 usd in rub"), &r), Some(("8000,00 RUB".into(), "03.11".into())));
        assert_eq!(convert_with(&q("92 rub to eur"), &r), Some(("1,00 EUR".into(), "03.11".into())));
        assert_eq!(convert_with(&q("1 eur в usd"), &r), Some(("1,15 USD".into(), "03.11".into())));
        assert_eq!(convert_with(&q("100 jpy to rub"), &r).unwrap().0, "52,00 RUB");
    }

    #[test]
    fn words_and_signs_resolve_to_codes() {
        assert_eq!(code_of("USD"), Some("USD"));
        assert_eq!(code_of("долларов"), Some("USD"));
        assert_eq!(code_of("$"), Some("USD"));
        assert_eq!(code_of("€"), Some("EUR"));
        assert_eq!(code_of("₽"), Some("RUB"));
        assert_eq!(code_of("Руб."), Some("RUB"));
        assert_eq!(code_of("фунтов стерлингов"), Some("GBP"));
        assert_eq!(code_of("юань"), Some("CNY"));
        assert_eq!(code_of("kzt"), Some("KZT"));
        assert_eq!(code_of("фунт"), None, "a bare pound stays a unit of mass");
        assert_eq!(code_of("km"), None);
        assert_eq!(code_of(""), None);
    }

    #[test]
    fn aliases_never_collide_with_units_or_each_other() {
        let mut seen: Vec<&str> = Vec::new();
        for (_, names) in ALIASES {
            for n in *names {
                assert!(!seen.contains(n), "alias {n:?} used twice");
                seen.push(n);
                assert!(units::convert(&format!("1 {n} in {n}")).is_none(), "{n:?} is also a unit");
            }
        }
    }

    #[test]
    fn small_and_large_amounts_are_formatted_sensibly() {
        assert_eq!(format_money(1234.5), "1234,50");
        assert_eq!(format_money(0.012345), "0,0123");
        assert_eq!(format_money(-3.0), "-3,00");
    }

    #[test]
    #[ignore] // ходит в сеть
    fn real_bank_answer_downloads_and_parses() {
        let body = crate::http::get(URL, TIMEOUT_MS).expect("download");
        let r = parse(&body).expect("parse");
        println!("{} USD={} EUR={} codes={}", r.date, r.rub["USD"], r.rub["EUR"], r.rub.len());
        assert!(r.rub["USD"] > 1.0);
    }

    #[test]
    fn queries_without_currencies_are_not_currency() {
        assert_eq!(convert("10 km in mi"), Outcome::NotCurrency);
        assert_eq!(convert("hello"), Outcome::NotCurrency);
        assert_eq!(convert("100 usd"), Outcome::NotCurrency);
    }
}
