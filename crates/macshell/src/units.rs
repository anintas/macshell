//! Конвертация единиц для Spotlight: `10 km in mi`, `100 f to c`, `5 фунтов в кг`, `2 gb в mb`. Длина, масса,
//! объём, площадь, скорость, время, данные, давление и температура. Валюты здесь нет: им нужны курсы из сети.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cat {
    Length,
    Mass,
    Volume,
    Area,
    Speed,
    Time,
    Data,
    Pressure,
    Temperature,
}

struct Unit {
    cat: Cat,
    /// Названия в нижнем регистре: символы, слова, русские формы.
    names: &'static [&'static str],
    /// Подпись результата на английском и русском.
    en: &'static str,
    ru: &'static str,
    /// Сколько базовых единиц категории в одной такой. У температуры не используется.
    k: f64,
}

const fn u(cat: Cat, names: &'static [&'static str], en: &'static str, ru: &'static str, k: f64) -> Unit {
    Unit { cat, names, en, ru, k }
}

/// Базы: длина м, масса кг, объём л, площадь м², скорость м/с, время с, данные байт, давление Па.
const UNITS: &[Unit] = &[
    u(Cat::Length, &["mm", "millimeter", "millimeters", "миллиметр", "миллиметра", "миллиметров", "мм"], "mm", "мм", 0.001),
    u(Cat::Length, &["cm", "centimeter", "centimeters", "сантиметр", "сантиметра", "сантиметров", "см"], "cm", "см", 0.01),
    u(Cat::Length, &["m", "meter", "meters", "metre", "metres", "метр", "метра", "метров", "м"], "m", "м", 1.0),
    u(Cat::Length, &["km", "kilometer", "kilometers", "kilometre", "километр", "километра", "километров", "км"], "km", "км", 1000.0),
    u(Cat::Length, &["in", "inch", "inches", "\"", "дюйм", "дюйма", "дюймов"], "in", "дюйм", 0.0254),
    u(Cat::Length, &["ft", "foot", "feet", "фут", "фута", "футов"], "ft", "фт", 0.3048),
    u(Cat::Length, &["yd", "yard", "yards", "ярд", "ярда", "ярдов"], "yd", "ярд", 0.9144),
    u(Cat::Length, &["mi", "mile", "miles", "миля", "мили", "миль"], "mi", "миль", 1609.344),
    u(Cat::Length, &["nmi", "nautical mile", "nautical miles", "морская миля", "морских миль", "морские мили"], "nmi", "мор. миль", 1852.0),
    u(Cat::Mass, &["mg", "milligram", "milligrams", "миллиграмм", "миллиграмма", "мг"], "mg", "мг", 1e-6),
    u(Cat::Mass, &["g", "gram", "grams", "грамм", "грамма", "граммов", "г", "гр"], "g", "г", 0.001),
    u(Cat::Mass, &["kg", "kilogram", "kilograms", "килограмм", "килограмма", "килограммов", "кг", "кило"], "kg", "кг", 1.0),
    u(Cat::Mass, &["t", "tonne", "tonnes", "ton", "tons", "тонна", "тонны", "тонн", "т"], "t", "т", 1000.0),
    u(Cat::Mass, &["oz", "ounce", "ounces", "унция", "унции", "унций"], "oz", "унц.", 0.028349523125),
    u(Cat::Mass, &["lb", "lbs", "pound", "pounds", "фунт", "фунта", "фунтов"], "lb", "фунт", 0.45359237),
    u(Cat::Volume, &["ml", "milliliter", "milliliters", "millilitre", "миллилитр", "миллилитра", "миллилитров", "мл"], "ml", "мл", 0.001),
    u(Cat::Volume, &["l", "liter", "liters", "litre", "litres", "литр", "литра", "литров", "л"], "l", "л", 1.0),
    u(Cat::Volume, &["m3", "m³", "cubic meter", "cubic meters", "куб м", "кубометр", "кубометров"], "m³", "м³", 1000.0),
    u(Cat::Volume, &["gal", "gallon", "gallons", "галлон", "галлона", "галлонов"], "gal", "гал.", 3.785411784),
    u(Cat::Volume, &["qt", "quart", "quarts", "кварта", "кварты", "кварт"], "qt", "кварт", 0.946352946),
    u(Cat::Volume, &["pt", "pint", "pints", "пинта", "пинты", "пинт"], "pt", "пинт", 0.473176473),
    u(Cat::Volume, &["cup", "cups", "чашка", "чашки", "чашек", "стакан", "стакана", "стаканов"], "cup", "чашк.", 0.2365882365),
    u(Cat::Volume, &["fl oz", "floz", "fluid ounce", "fluid ounces", "жидкая унция", "жидких унций"], "fl oz", "жидк. унц.", 0.0295735295625),
    u(Cat::Volume, &["tbsp", "tablespoon", "tablespoons", "столовая ложка", "столовые ложки", "столовых ложек", "ст л"], "tbsp", "ст. л.", 0.01478676478125),
    u(Cat::Volume, &["tsp", "teaspoon", "teaspoons", "чайная ложка", "чайные ложки", "чайных ложек", "ч л"], "tsp", "ч. л.", 0.00492892159375),
    u(Cat::Area, &["mm2", "mm²", "sq mm", "кв мм", "мм2", "мм²"], "mm²", "мм²", 1e-6),
    u(Cat::Area, &["cm2", "cm²", "sq cm", "кв см", "см2", "см²"], "cm²", "см²", 1e-4),
    u(Cat::Area, &["m2", "m²", "sq m", "square meter", "square meters", "кв м", "м2", "м²", "квадратный метр", "квадратных метров"], "m²", "м²", 1.0),
    u(Cat::Area, &["km2", "km²", "sq km", "square kilometer", "кв км", "км2", "км²"], "km²", "км²", 1e6),
    u(Cat::Area, &["ha", "hectare", "hectares", "га", "гектар", "гектара", "гектаров"], "ha", "га", 10_000.0),
    u(Cat::Area, &["acre", "acres", "акр", "акра", "акров"], "acre", "акр", 4046.8564224),
    u(Cat::Area, &["ft2", "ft²", "sq ft", "square foot", "square feet", "кв фут", "фут2", "фут²"], "ft²", "фт²", 0.09290304),
    u(Cat::Area, &["in2", "in²", "sq in", "square inch", "square inches", "кв дюйм"], "in²", "дюйм²", 0.00064516),
    u(Cat::Area, &["mi2", "mi²", "sq mi", "square mile", "square miles", "кв миль"], "mi²", "миль²", 2_589_988.110336),
    u(Cat::Speed, &["m/s", "mps", "м/с", "метр в секунду", "метров в секунду"], "m/s", "м/с", 1.0),
    u(Cat::Speed, &["km/h", "kmh", "kph", "км/ч", "километров в час", "километр в час"], "km/h", "км/ч", 1.0 / 3.6),
    u(Cat::Speed, &["mph", "mi/h", "миль/ч", "миль в час", "мили в час"], "mph", "миль/ч", 0.44704),
    u(Cat::Speed, &["kn", "kt", "knot", "knots", "узел", "узла", "узлов"], "kn", "уз", 0.514444444),
    u(Cat::Speed, &["ft/s", "fps", "фут/с", "футов в секунду"], "ft/s", "фт/с", 0.3048),
    u(Cat::Time, &["ms", "millisecond", "milliseconds", "мс", "миллисекунда", "миллисекунд"], "ms", "мс", 0.001),
    u(Cat::Time, &["s", "sec", "secs", "second", "seconds", "с", "сек", "секунда", "секунды", "секунд"], "s", "с", 1.0),
    u(Cat::Time, &["min", "mins", "minute", "minutes", "мин", "минута", "минуты", "минут"], "min", "мин", 60.0),
    u(Cat::Time, &["h", "hr", "hrs", "hour", "hours", "ч", "час", "часа", "часов"], "h", "ч", 3600.0),
    u(Cat::Time, &["d", "day", "days", "день", "дня", "дней", "сутки", "суток"], "d", "сут", 86_400.0),
    u(Cat::Time, &["wk", "week", "weeks", "неделя", "недели", "недель"], "wk", "нед", 604_800.0),
    u(Cat::Time, &["yr", "year", "years", "год", "года", "лет"], "yr", "лет", 31_557_600.0),
    u(Cat::Data, &["bit", "bits", "бит", "бита", "битов"], "bit", "бит", 0.125),
    u(Cat::Data, &["byte", "bytes", "b", "байт", "байта", "байтов", "б"], "B", "Б", 1.0),
    u(Cat::Data, &["kb", "kilobyte", "kilobytes", "кб", "килобайт", "килобайта", "килобайтов"], "KB", "КБ", 1e3),
    u(Cat::Data, &["mb", "megabyte", "megabytes", "мб", "мегабайт", "мегабайта", "мегабайтов"], "MB", "МБ", 1e6),
    u(Cat::Data, &["gb", "gigabyte", "gigabytes", "гб", "гигабайт", "гигабайта", "гигабайтов"], "GB", "ГБ", 1e9),
    u(Cat::Data, &["tb", "terabyte", "terabytes", "тб", "терабайт", "терабайта", "терабайтов"], "TB", "ТБ", 1e12),
    u(Cat::Data, &["kib", "kibibyte", "кибибайт"], "KiB", "КиБ", 1024.0),
    u(Cat::Data, &["mib", "mebibyte", "мебибайт"], "MiB", "МиБ", 1_048_576.0),
    u(Cat::Data, &["gib", "gibibyte", "гибибайт"], "GiB", "ГиБ", 1_073_741_824.0),
    u(Cat::Data, &["tib", "tebibyte", "тебибайт"], "TiB", "ТиБ", 1_099_511_627_776.0),
    u(Cat::Pressure, &["pa", "pascal", "pascals", "па", "паскаль", "паскаля", "паскалей"], "Pa", "Па", 1.0),
    u(Cat::Pressure, &["kpa", "kilopascal", "кпа", "килопаскаль", "килопаскаля"], "kPa", "кПа", 1000.0),
    u(Cat::Pressure, &["bar", "bars", "бар", "бара", "баров"], "bar", "бар", 100_000.0),
    u(Cat::Pressure, &["atm", "atmosphere", "atmospheres", "атм", "атмосфера", "атмосферы", "атмосфер"], "atm", "атм", 101_325.0),
    u(Cat::Pressure, &["psi", "пси"], "psi", "psi", 6894.757293168),
    u(Cat::Pressure, &["mmhg", "mm hg", "torr", "мм рт ст", "мм рт. ст.", "торр"], "mmHg", "мм рт. ст.", 133.322387415),
    u(Cat::Temperature, &["c", "°c", "celsius", "degc", "° c", "градус цельсия", "градусов цельсия", "цельсий", "цельсия"], "°C", "°C", 1.0),
    u(Cat::Temperature, &["f", "°f", "fahrenheit", "degf", "° f", "градус фаренгейта", "градусов фаренгейта", "фаренгейт", "фаренгейта"], "°F", "°F", 1.0),
    u(Cat::Temperature, &["k", "kelvin", "kelvins", "кельвин", "кельвина", "кельвинов"], "K", "К", 1.0),
];

/// Слова между числом с единицей и целевой единицей.
const SEPARATORS: &[&str] = &["in", "to", "into", "as", "в", "на", "->", "=>", "→", "="];

fn find_unit(name: &str) -> Option<&'static Unit> {
    let n = name.trim().trim_end_matches('.');
    UNITS.iter().find(|u| u.names.contains(&n))
}

/// Температура в градусах Цельсия из `value` единицы `unit` (по первому символу подписи).
fn to_celsius(value: f64, unit: &Unit) -> f64 {
    match unit.en {
        "°F" => (value - 32.0) / 1.8,
        "K" => value - 273.15,
        _ => value,
    }
}

fn from_celsius(celsius: f64, unit: &Unit) -> f64 {
    match unit.en {
        "°F" => celsius * 1.8 + 32.0,
        "K" => celsius + 273.15,
        _ => celsius,
    }
}

/// Число из начала запроса и остаток: `-12,5 km` даёт `(-12.5, "km")`. Допускается и `10km` без пробела.
fn leading_number(s: &str) -> Option<(f64, &str)> {
    let s = s.trim_start();
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let ok = c.is_ascii_digit() || ((c == '.' || c == ',') && i > 0) || (i == 0 && (c == '-' || c == '+'));
        if !ok {
            break;
        }
        end = i + c.len_utf8();
    }
    let text = s[..end].replace(',', ".");
    let value: f64 = text.parse().ok()?;
    Some((value, s[end..].trim_start()))
}

/// Число с шестью значащими цифрами, запятая как десятичный знак, без хвостовых нулей.
pub fn format_value(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if !v.is_finite() {
        return String::new();
    }
    let magnitude = v.abs().log10().floor() as i32;
    if !(-6..=15).contains(&magnitude) {
        return format!("{v:.5e}").replace('.', ",");
    }
    let decimals = (5 - magnitude).clamp(0, 12) as usize;
    let text = format!("{v:.decimals$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text };
    text.replace('.', ",")
}

/// Запрос «число что (in|to|в|на) во что», разобранный на части; единицы или валюты определяет вызывающий.
#[derive(Clone, Debug, PartialEq)]
pub struct ConversionQuery {
    pub value: f64,
    pub from: String,
    pub to: String,
    /// В запросе есть русские буквы: ответ подписывается по-русски.
    pub russian: bool,
}

/// Разбирает запрос вида «10 km in mi». `None`, если числа или разделителя нет.
pub fn parse_query(query: &str) -> Option<ConversionQuery> {
    let lower = query.trim().to_lowercase();
    let (value, rest) = leading_number(&lower)?;
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    // Разделитель ищется со второго слова: первое всегда начало исходной единицы (`in` это ещё и дюйм).
    let sep = tokens.iter().enumerate().skip(1).find(|(_, t)| SEPARATORS.contains(t)).map(|(i, _)| i)?;
    let (from, to) = (tokens[..sep].join(" "), tokens[sep + 1..].join(" "));
    if from.is_empty() || to.is_empty() {
        return None;
    }
    let russian = lower.chars().any(|c| ('а'..='я').contains(&c) || c == 'ё');
    Some(ConversionQuery { value, from, to, russian })
}

/// Результат конвертации запроса вида «число единица (in|to|в|на) единица» в виде «6,21371 mi». `None`, если
/// запрос другого вида, единицы неизвестны или из разных категорий.
pub fn convert(query: &str) -> Option<String> {
    let q = parse_query(query)?;
    let from = find_unit(&q.from)?;
    let to = find_unit(&q.to)?;
    if from.cat != to.cat {
        return None;
    }
    let result =
        if from.cat == Cat::Temperature { from_celsius(to_celsius(q.value, from), to) } else { q.value * from.k / to.k };
    let number = format_value(result);
    if number.is_empty() {
        return None;
    }
    Some(format!("{number} {}", if q.russian { to.ru } else { to.en }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(q: &str) -> Option<String> {
        convert(q)
    }

    #[test]
    fn common_length_mass_and_volume_conversions() {
        assert_eq!(c("10 km in mi").as_deref(), Some("6,21371 mi"));
        assert_eq!(c("1 mi to km").as_deref(), Some("1,60934 km"));
        assert_eq!(c("5 ft to cm").as_deref(), Some("152,4 cm"));
        assert_eq!(c("2,5 kg in lb").as_deref(), Some("5,51156 lb"));
        assert_eq!(c("1 gal in l").as_deref(), Some("3,78541 l"));
        assert_eq!(c("1 cup in ml").as_deref(), Some("236,588 ml"));
    }

    #[test]
    fn inch_is_not_mistaken_for_the_separator() {
        assert_eq!(c("5 in in cm").as_deref(), Some("12,7 cm"));
        assert_eq!(c("12.7 cm to in").as_deref(), Some("5 in"));
    }

    #[test]
    fn temperature_uses_offsets_not_factors() {
        assert_eq!(c("100 f to c").as_deref(), Some("37,7778 °C"));
        assert_eq!(c("0 c in f").as_deref(), Some("32 °F"));
        assert_eq!(c("0 c to k").as_deref(), Some("273,15 K"));
        assert_eq!(c("-40 c in f").as_deref(), Some("-40 °F"));
    }

    #[test]
    fn data_speed_time_and_pressure() {
        assert_eq!(c("1 gb in mb").as_deref(), Some("1000 MB"));
        assert_eq!(c("1 gib to mib").as_deref(), Some("1024 MiB"));
        assert_eq!(c("100 km/h to m/s").as_deref(), Some("27,7778 m/s"));
        assert_eq!(c("2 h in min").as_deref(), Some("120 min"));
        assert_eq!(c("1 atm to pa").as_deref(), Some("101325 Pa"));
        assert_eq!(c("1 ha to m2").as_deref(), Some("10000 m²"));
    }

    #[test]
    fn russian_words_and_separators_answer_in_russian() {
        assert_eq!(c("10 км в мили").as_deref(), Some("6,21371 миль"));
        assert_eq!(c("5 фунтов в кг").as_deref(), Some("2,26796 кг"));
        assert_eq!(c("1 литр на мл").as_deref(), Some("1000 мл"));
        assert_eq!(c("32 f в c").as_deref(), Some("0 °C"));
    }

    #[test]
    fn glued_numbers_and_multiword_units_work() {
        assert_eq!(c("10km in m").as_deref(), Some("10000 m"));
        assert_eq!(c("8 fl oz to ml").as_deref(), Some("236,588 ml"));
        assert_eq!(c("1 nautical mile in km").as_deref(), Some("1,852 km"));
    }

    #[test]
    fn not_a_conversion_gives_nothing() {
        assert!(c("10 km").is_none(), "no target unit");
        assert!(c("10 kg in m").is_none(), "different categories");
        assert!(c("abc in def").is_none());
        assert!(c("km in mi").is_none(), "no number");
        assert!(c("10 foo in bar").is_none());
        assert!(c("2+2").is_none());
        assert!(c("").is_none());
    }

    #[test]
    fn every_alias_is_unique_across_the_table() {
        let mut seen: Vec<(&str, Cat)> = Vec::new();
        for unit in UNITS {
            for name in unit.names {
                if let Some((_, cat)) = seen.iter().find(|(n, _)| n == name) {
                    panic!("alias {name:?} used twice (also in {cat:?})");
                }
                seen.push((name, unit.cat));
            }
        }
    }

    #[test]
    fn number_formatting_keeps_six_significant_digits() {
        assert_eq!(format_value(6.213711922), "6,21371");
        assert_eq!(format_value(1000.0), "1000");
        assert_eq!(format_value(0.000123456789), "0,000123457");
        assert_eq!(format_value(1_234_567.89), "1234568");
        assert_eq!(format_value(-2.5), "-2,5");
        assert_eq!(format_value(0.0), "0");
    }
}
