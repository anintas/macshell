//! Значки состояния в правой части панели: какие показывать и в каком порядке (Menu Bar settings и перестановка
//! значков в macOS). Порядок и скрытые хранятся в конфиге словами (`bar_order`, `bar_hidden`), панель получает готовый
//! список номеров. Всё здесь чистые функции.

/// Виды значков; номер в этом списке и есть номер вида в `topbar.slint`.
pub const KINDS: [&str; 10] = ["battery", "focus", "privacy", "vpn", "bluetooth", "lang", "volume", "wifi", "spotlight", "control"];

pub fn title(kind: usize) -> &'static str {
    ["Батарея", "Не беспокоить", "Микрофон и камера", "VPN", "Bluetooth", "Раскладка клавиатуры", "Звук", "Сеть", "Spotlight", "Центр управления"]
        .get(kind)
        .copied()
        .unwrap_or("")
}

fn kind_of(name: &str) -> Option<usize> {
    KINDS.iter().position(|k| k.eq_ignore_ascii_case(name.trim()))
}

/// Порядок слева направо: названия из конфига, незнакомые и повторы пропускаются, недостающие виды идут следом в
/// обычном порядке. Пустая строка даёт обычный порядок.
pub fn parse_order(text: &str) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    for kind in text.split(',').filter_map(kind_of) {
        if !order.contains(&kind) {
            order.push(kind);
        }
    }
    for kind in 0..KINDS.len() {
        if !order.contains(&kind) {
            order.push(kind);
        }
    }
    order
}

/// Скрытые виды из строки конфига.
pub fn parse_hidden(text: &str) -> Vec<usize> {
    let mut hidden: Vec<usize> = text.split(',').filter_map(kind_of).collect();
    hidden.sort_unstable();
    hidden.dedup();
    hidden
}

pub fn serialize(kinds: &[usize]) -> String {
    kinds.iter().filter_map(|k| KINDS.get(*k)).copied().collect::<Vec<_>>().join(",")
}

/// Обычный ли порядок: тогда в конфиг ничего не пишется.
pub fn is_default_order(order: &[usize]) -> bool {
    order.iter().copied().eq(0..KINDS.len())
}

/// На сколько мест сдвигать значок, который утащили на `dx` пикселей при ширине значка `unit`: округляется до целого,
/// сдвиг меньше полузначка остаётся на месте.
pub fn drag_steps(dx: f32, unit: f32) -> i32 {
    if unit <= 0.0 { 0 } else { (dx / unit).round() as i32 }
}

/// Номера, которые показывает панель: по порядку, только скрытые не в списке и только те, что сейчас есть
/// (у компьютера без батареи значка батареи нет). `active[kind]`: значок вообще существует сейчас.
pub fn visible(order: &[usize], hidden: &[usize], active: &[bool; 10]) -> Vec<i32> {
    order.iter().copied().filter(|k| !hidden.contains(k) && active.get(*k).copied().unwrap_or(false)).map(|k| k as i32).collect()
}

/// Сдвигает значок на соседнее видимое место: влево (`-1`) или вправо (`1`). Скрытые и отсутствующие значки
/// перешагиваются, чтобы сдвиг был виден.
pub fn move_visible(order: &[usize], shown: &[bool; 10], kind: usize, delta: i32) -> Vec<usize> {
    let mut out = order.to_vec();
    let Some(from) = out.iter().position(|k| *k == kind) else { return out };
    let mut to = from as i32 + delta.signum();
    while to >= 0 && (to as usize) < out.len() && !shown[out[to as usize]] {
        to += delta.signum();
    }
    if to >= 0 && (to as usize) < out.len() {
        out.swap(from, to as usize);
    }
    out
}

/// Включает значок в список скрытых или убирает из него.
pub fn toggle_hidden(hidden: &[usize], kind: usize) -> Vec<usize> {
    let mut out: Vec<usize> = hidden.to_vec();
    if let Some(i) = out.iter().position(|k| *k == kind) {
        out.remove(i);
    } else {
        out.push(kind);
        out.sort_unstable();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_moves_by_whole_icon_widths() {
        assert_eq!((drag_steps(10.0, 24.0), drag_steps(13.0, 24.0), drag_steps(-50.0, 24.0), drag_steps(30.0, 0.0)), (0, 1, -2, 0));
    }

    #[test]
    fn order_keeps_listed_names_then_fills_in_the_rest() {
        assert_eq!(parse_order(""), (0..10).collect::<Vec<_>>());
        let o = parse_order("wifi, Battery, bogus, wifi");
        assert_eq!(&o[..2], &[7, 0]);
        assert_eq!(o.len(), 10, "every kind appears once");
        assert!(!is_default_order(&o));
        assert!(is_default_order(&parse_order("")));
    }

    #[test]
    fn serialize_roundtrips() {
        let order = parse_order("control,spotlight");
        assert_eq!(parse_order(&serialize(&order)), order);
        assert_eq!(parse_hidden(&serialize(&[7, 2])), vec![2, 7]);
        assert!(parse_hidden("").is_empty());
    }

    #[test]
    fn panel_gets_only_shown_and_existing_icons_in_order() {
        let order = parse_order("wifi,battery");
        let mut active = [true; 10];
        active[0] = false; // нет батареи
        assert_eq!(visible(&order, &[8], &active)[..2], [7, 1], "battery is absent, spotlight hidden");
        assert!(!visible(&order, &[8], &active).contains(&8));
        assert!(!visible(&order, &[], &active).contains(&0));
    }

    #[test]
    fn moving_steps_over_hidden_neighbours() {
        let order: Vec<usize> = (0..10).collect();
        let mut shown = [true; 10];
        shown[5] = false;
        shown[6] = false;
        // Bluetooth (4) вправо: ближайший видимый сосед после скрытых 5 и 6 это Сеть (7).
        let moved = move_visible(&order, &shown, 4, 1);
        assert_eq!(moved[4], 7);
        assert_eq!(moved[7], 4);
        // У края сдвиг ничего не меняет.
        assert_eq!(move_visible(&order, &[true; 10], 0, -1), order);
        assert_eq!(move_visible(&order, &[true; 10], 9, 1), order);
    }

    #[test]
    fn toggling_hides_and_shows_again() {
        let hidden = toggle_hidden(&[], 7);
        assert_eq!(hidden, vec![7]);
        assert_eq!(toggle_hidden(&hidden, 3), vec![3, 7]);
        assert!(toggle_hidden(&hidden, 7).is_empty());
    }
}
