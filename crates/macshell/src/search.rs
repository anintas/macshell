//! Поиск для Spotlight: нормализация, перевод набранного в чужой раскладке, оценка совпадения, калькулятор.
//! Всё чистые функции без системных вызовов.

/// Строчные буквы, `ё` как `е`, лишние пробелы убраны.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        for c in word.chars().flat_map(char::to_lowercase) {
            out.push(if c == 'ё' { 'е' } else { c });
        }
    }
    out
}

/// Клавиши ЙЦУКЕН и QWERTY в одном порядке: набранное в одной раскладке читается в другой.
const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";

/// Что получилось бы, если бы запрос набрали в другой раскладке: `ыщгтв` даёт `sound`, `sound` даёт `ыщгтв`.
/// Каждый символ переводится сам: буква латиницы в русскую, русская в латинскую, остальное не меняется.
pub fn swap_layout(s: &str) -> String {
    let en: Vec<char> = EN.chars().collect();
    let ru: Vec<char> = RU.chars().collect();
    s.chars()
        .map(|c| {
            if let Some(i) = en.iter().position(|&e| e == c) {
                ru[i]
            } else if let Some(i) = ru.iter().position(|&r| r == c) {
                en[i]
            } else {
                c
            }
        })
        .collect()
}

fn words(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = None;
    let mut parts = Vec::new();
    for (i, c) in s.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(st) = start.take() {
            parts.push((st, &s[st..i]));
        }
    }
    if let Some(st) = start {
        parts.push((st, &s[st..]));
    }
    parts.into_iter()
}

/// Оценка одного слова запроса против названия. Оба уже нормализованы. Больше значит лучше.
fn score_token(token: &str, title: &str) -> Option<i32> {
    let (tl, ll) = (token.chars().count() as i32, title.chars().count() as i32);
    if title == token {
        return Some(1000);
    }
    if title.starts_with(token) {
        return Some(900 - (ll - tl).min(99));
    }
    if let Some((n, _)) = words(title).enumerate().find(|(_, (_, w))| w.starts_with(token)).map(|(n, w)| (n, w)) {
        return Some(700 - (n as i32 * 10).min(90));
    }
    if let Some(pos) = title.find(token) {
        return Some(500 - (title[..pos].chars().count() as i32).min(99));
    }
    if tl >= 2 {
        // Первые буквы слов подряд: `vsc` находит Visual Studio Code.
        let initials: String = words(title).filter_map(|(_, w)| w.chars().next()).collect();
        if initials.contains(token) {
            return Some(450);
        }
        // Буквы по порядку с небольшими разрывами.
        let mut chars = title.chars().enumerate();
        let (mut first, mut last) = (None, 0);
        for t in token.chars() {
            let (i, _) = chars.find(|&(_, c)| c == t)?;
            first.get_or_insert(i);
            last = i;
        }
        let span = (last - first? + 1) as i32;
        if span <= 2 * tl + 2 {
            return Some(150 - (span - tl) * 5);
        }
    }
    None
}

/// Оценка названия против запроса из одного или нескольких слов; все слова должны найтись.
/// `query` и `title` уже нормализованы (`normalize`).
pub fn score(query: &str, title: &str) -> Option<i32> {
    let mut sum = 0;
    let mut n = 0;
    for token in query.split(' ').filter(|t| !t.is_empty()) {
        sum += score_token(token, title)?;
        n += 1;
    }
    (n > 0).then(|| sum / n)
}

/// Лучшая оценка запроса `raw` против названия и слов поиска (оба уже нормализованы). Запрос пробуется и в
/// другой раскладке (штраф 100); совпадение по слову поиска слабее совпадения по названию на 150.
pub fn match_score(raw: &str, title_norm: &str, keywords: &[String]) -> Option<i32> {
    let q = normalize(raw);
    let swapped = swap_layout(&q);
    let variants: Vec<(&str, i32)> =
        if swapped == q { vec![(q.as_str(), 0)] } else { vec![(q.as_str(), 0), (swapped.as_str(), -100)] };
    variants
        .iter()
        .filter_map(|&(v, penalty)| {
            let by_title = score(v, title_norm);
            let by_keyword = keywords.iter().filter_map(|k| score(v, k)).max().map(|s| s - 150);
            Some(by_title.into_iter().chain(by_keyword).max()? + penalty)
        })
        .max()
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl Parser<'_> {
    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.chars.next();
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        while let Some(op @ ('*' | '/' | 'x' | '×' | '÷' | '·')) = self.peek() {
            self.chars.next();
            let r = self.power()?;
            v = if matches!(op, '/' | '÷') { v / r } else { v * r };
        }
        Some(v)
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.peek() == Some('^') {
            self.chars.next();
            return Some(base.powf(self.power()?));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek()? {
            '-' => {
                self.chars.next();
                Some(-self.unary()?)
            }
            '+' => {
                self.chars.next();
                self.unary()
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Option<f64> {
        if self.peek()? == '(' {
            self.chars.next();
            let v = self.expr()?;
            return (self.chars.next()? == ')').then_some(v);
        }
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                text.push(c);
            } else if (c == '.' || c == ',') && !text.contains('.') {
                text.push('.');
            } else {
                break;
            }
            self.chars.next();
        }
        if text.is_empty() || text == "." { None } else { text.parse().ok() }
    }
}

/// Форматирует число как в русской локали: запятая, без хвостовых нулей.
fn format_number(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if v.abs() < 1e15 && (v - v.round()).abs() < 1e-9 {
        return format!("{}", v.round() as i64);
    }
    let text = format!("{:.10}", v);
    text.trim_end_matches('0').trim_end_matches('.').replace('.', ",")
}

/// Значение выражения (`2+2*3`, `(1+2)^2`, `7,5/2`), если запрос арифметический. Одно число не выражение.
pub fn calc(query: &str) -> Option<String> {
    let text: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    if text.is_empty() || !text.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut p = Parser { chars: text.chars().peekable() };
    let v = p.expr()?;
    if p.peek().is_some() || !v.is_finite() {
        return None;
    }
    // Одно число (с минусом или без) показывать нечего.
    let bare = text.trim_start_matches(['-', '+']);
    if bare.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') {
        return None;
    }
    Some(format_number(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lowercases_folds_yo_and_trims() {
        assert_eq!(normalize("  Ёлка   Настройки  "), "елка настройки");
        assert_eq!(normalize("Visual STUDIO"), "visual studio");
    }

    #[test]
    fn swap_layout_translates_both_ways() {
        assert_eq!(swap_layout("ыщгтв"), "sound");
        assert_eq!(swap_layout("sound"), "ыщгтв");
        assert_eq!(swap_layout("цшаш"), "wifi");
        assert_eq!(swap_layout("123"), "123");
        assert_eq!(swap_layout("ю"), ".");
    }

    #[test]
    fn score_orders_match_kinds() {
        let exact = score("звук", "звук").unwrap();
        let prefix = score("звук", "звукозапись").unwrap();
        let word = score("звук", "настройки звука").unwrap();
        let sub = score("вук", "настройки звука").unwrap();
        let sparse = score("нтк", "настройки звука").unwrap();
        assert!(exact > prefix && prefix > word && word > sub && sub > sparse, "{exact} {prefix} {word} {sub} {sparse}");
        assert!(score("яблоко", "настройки звука").is_none());
    }

    #[test]
    fn score_matches_initials_of_words() {
        assert!(score("vsc", "visual studio code").is_some());
        assert!(score("vsx", "visual studio code").is_none());
    }

    #[test]
    fn multi_word_query_needs_every_word() {
        assert!(score("настр звук", "настройки звука").is_some());
        assert!(score("настр экран", "настройки звука").is_none());
    }

    #[test]
    fn shorter_title_wins_on_equal_prefix() {
        assert!(score("наст", "настройки").unwrap() > score("наст", "настройки звука и экрана").unwrap());
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(score("", "настройки").is_none());
    }

    #[test]
    fn calc_basic_precedence_and_parentheses() {
        assert_eq!(calc("2+2*3").as_deref(), Some("8"));
        assert_eq!(calc("(1+2)^2").as_deref(), Some("9"));
        assert_eq!(calc("2^3^2").as_deref(), Some("512")); // степень правоассоциативна
        assert_eq!(calc("10 - 4 - 3").as_deref(), Some("3"));
        assert_eq!(calc("-3+5").as_deref(), Some("2"));
        assert_eq!(calc("6×7").as_deref(), Some("42"));
        assert_eq!(calc("2x3").as_deref(), Some("6"));
    }

    #[test]
    fn calc_uses_comma_decimals() {
        assert_eq!(calc("7,5/2").as_deref(), Some("3,75"));
        assert_eq!(calc("0.1+0.2").as_deref(), Some("0,3"));
        assert_eq!(calc("1/3").as_deref(), Some("0,3333333333"));
    }

    #[test]
    fn calc_rejects_non_expressions() {
        for q in ["abc", "5", "-5", "", "2+", "(2+3", "2+3)", "x", "xbox 360", "1/0", "12:30", "3,1,4"] {
            assert_eq!(calc(q), None, "{q:?}");
        }
    }
}
