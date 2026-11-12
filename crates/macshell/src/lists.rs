//! Данные для страниц со списком (Bluetooth-устройства, сети Wi-Fi) в Центре управления и в окнах под значками
//! строки меню. Здесь же размеры: высота окна считается в Rust и должна совпадать с `panel-height` в
//! `listpanel.slint` (144 плюс тело; тело 22 плюс 44 на строку или 100, когда показывать нечего).

use std::time::{Duration, Instant};

use crate::bluetooth::{self, Device};
use crate::slint_view::ListRow;
use crate::wifi::{self, WifiNet};

/// Высота тела страницы: заголовок раздела 22 и строки по 44 или пустое место 100.
pub fn body(rows: usize) -> f32 {
    if rows > 0 { 22.0 + 44.0 * rows as f32 } else { 100.0 }
}

/// Высота отдельного окна со списком из `rows` строк.
pub fn height(rows: usize) -> f32 {
    144.0 + body(rows)
}

/// Сколько строк помещается в тело высотой `body` логических пикселей (не меньше одной).
pub fn max_rows(body: f32) -> usize {
    (((body - 22.0) / 44.0).floor() as i64).max(1) as usize
}

/// Что показывает страница.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub title: &'static str,
    pub heading: &'static str,
    pub on: bool,
    pub available: bool,
    pub state: &'static str,
    pub rows: Vec<ListRow>,
    pub empty: String,
    pub footer: String,
}

/// Нажатие на строку: что делать окну.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Nothing,
    /// Открыть параметры Windows (страница `ms-settings:`) и закрыть окно.
    Settings,
    /// Открыть окно выбора сети Windows и закрыть окно.
    Networks,
}

const POLL: Duration = Duration::from_secs(2);
const PENDING: Duration = Duration::from_secs(6);

fn footer(more: usize, plain: &str, with_more: &str) -> String {
    if more > 0 { format!("Ещё {more} {with_more}") } else { plain.to_string() }
}

// ---- Bluetooth

/// Подпись в шапке по состоянию радио (`bluetooth::ON` и т. д.).
pub fn bt_state_text(state: i32) -> &'static str {
    match state {
        bluetooth::ON => "Включён",
        bluetooth::OFF => "Выключен",
        _ => "Нет адаптера",
    }
}

/// Надписи строки устройства: (состояние, кнопка). `pending`: `Some(true)` подключаем, `Some(false)` отключаем.
/// Устройствам BLE Windows не даёт управлять соединением из программы, у них кнопки нет.
pub fn bt_texts(d: &Device, pending: Option<bool>) -> (&'static str, &'static str) {
    match (pending, d.connected) {
        (Some(true), _) => ("Подключение…", ""),
        (Some(false), _) => ("Отключение…", ""),
        (None, true) => ("Подключено", if d.le { "" } else { "Отключить" }),
        (None, false) => ("Не подключено", if d.le { "" } else { "Подключить" }),
    }
}

/// Список устройств: читается из системы, обновляется раз в пару секунд, помнит нажатие, которое система ещё
/// выполняет.
pub struct BtList {
    pub devices: Vec<Device>,
    /// Адрес, куда нажали, к какому состоянию идём и до какого времени ждём.
    pending: Option<(u64, bool, Instant)>,
    next_poll: Instant,
    pub max_rows: usize,
}

impl BtList {
    pub fn new(max_rows: usize) -> BtList {
        BtList::with_devices(bluetooth::devices(), max_rows)
    }

    /// Список с готовым набором устройств, без обращения к системе.
    pub fn with_devices(devices: Vec<Device>, max_rows: usize) -> BtList {
        BtList { devices, pending: None, next_poll: Instant::now() + POLL, max_rows }
    }

    pub fn page(&self) -> Page {
        let state = bluetooth::state();
        let on = state == bluetooth::ON;
        let rows: Vec<ListRow> = if on {
            self.devices
                .iter()
                .take(self.max_rows)
                .map(|d| {
                    let pending = self.pending.filter(|p| p.0 == d.address && !d.le).map(|p| p.1);
                    let (status, action) = bt_texts(d, pending);
                    ListRow { name: d.name.as_str().into(), status: status.into(), connected: d.connected, kind: bluetooth::kind_of(&d.name), action: action.into() }
                })
                .collect()
        } else {
            Vec::new()
        };
        let more = if on { self.devices.len().saturating_sub(self.max_rows) } else { 0 };
        let empty = match state {
            bluetooth::NONE => "В компьютере нет Bluetooth",
            bluetooth::OFF => "Включите Bluetooth, чтобы увидеть устройства",
            _ => "Нет сопряжённых устройств",
        };
        Page {
            title: "Bluetooth",
            heading: "Устройства",
            on,
            available: state != bluetooth::NONE,
            state: bt_state_text(state),
            rows,
            empty: empty.into(),
            footer: footer(more, "Параметры Bluetooth…", "в параметрах Bluetooth…"),
        }
    }

    /// Нажатие на строку: классическое устройство подключается или отключается, BLE открывает параметры.
    pub fn click(&mut self, i: usize) -> Click {
        let Some(d) = self.devices.get(i) else { return Click::Nothing };
        if d.le {
            return Click::Settings;
        }
        if self.pending.is_none() {
            let target = !d.connected;
            bluetooth::set_connected(d.address, target);
            self.pending = Some((d.address, target, Instant::now() + PENDING));
            self.next_poll = Instant::now() + Duration::from_millis(800);
        }
        Click::Nothing
    }

    /// Перечитывает список, когда подошло время. `true`, если строки изменились.
    pub fn poll(&mut self, force: bool) -> bool {
        let now = Instant::now();
        if !force && now < self.next_poll {
            return false;
        }
        self.next_poll = now + POLL;
        let fresh = bluetooth::devices_cached();
        if let Some((addr, target, until)) = self.pending {
            if fresh.iter().any(|d| d.address == addr && d.connected == target) || now >= until {
                self.pending = None;
            }
        }
        let changed = fresh != self.devices || self.pending.is_some();
        self.devices = fresh;
        changed
    }
}

// ---- Wi-Fi

/// Надписи строки сети: (состояние, кнопка).
pub fn wifi_texts(n: &WifiNet, pending: bool) -> (String, &'static str) {
    let signal = format!("{}%", n.signal);
    if pending {
        return ("Подключение…".into(), "");
    }
    if n.connected {
        (format!("Подключено · {signal}"), "")
    } else if n.known {
        (format!("Сохранена · {signal}"), "Подключить")
    } else if n.secured {
        (format!("Защищена · {signal}"), "Войти")
    } else {
        (format!("Открытая · {signal}"), "Войти")
    }
}

/// Список сетей Wi-Fi: радио и сети читаются раз в пару секунд.
pub struct WifiList {
    pub networks: Vec<WifiNet>,
    /// `None`, когда адаптера Wi-Fi нет.
    radio: Option<bool>,
    wired: bool,
    pending: Option<(String, Instant)>,
    next_poll: Instant,
    pub max_rows: usize,
}

impl WifiList {
    /// `wired`: сейчас подключена проводная сеть (подпись, когда адаптера Wi-Fi нет).
    pub fn new(max_rows: usize, wired: bool) -> WifiList {
        let radio = wifi::radio_on();
        let networks = if radio == Some(true) { wifi::networks() } else { Vec::new() };
        WifiList { networks, radio, wired, pending: None, next_poll: Instant::now() + POLL, max_rows }
    }

    pub fn radio(&self) -> Option<bool> {
        self.radio
    }

    pub fn page(&self) -> Page {
        let on = self.radio == Some(true);
        let rows: Vec<ListRow> = if on {
            self.networks
                .iter()
                .take(self.max_rows)
                .map(|n| {
                    let pending = self.pending.as_ref().is_some_and(|p| p.0 == n.ssid);
                    let (status, action) = wifi_texts(n, pending);
                    ListRow { name: n.ssid.as_str().into(), status: status.as_str().into(), connected: n.connected, kind: if n.secured { 9 } else { 8 }, action: action.into() }
                })
                .collect()
        } else {
            Vec::new()
        };
        let more = if on { self.networks.len().saturating_sub(self.max_rows) } else { 0 };
        let empty = match self.radio {
            None if self.wired => "Подключена проводная сеть",
            None => "В компьютере нет адаптера Wi-Fi",
            Some(false) => "Включите Wi-Fi, чтобы увидеть сети",
            Some(true) => "Сети не найдены",
        };
        Page {
            title: "Wi-Fi",
            heading: "Сети",
            on,
            available: self.radio.is_some(),
            state: match self.radio {
                Some(true) => "Включён",
                Some(false) => "Выключен",
                None => "Нет адаптера",
            },
            rows,
            empty: empty.into(),
            footer: footer(more, "Параметры сети…", "в параметрах сети…"),
        }
    }

    /// Известная сеть подключается кликом; новую (нужен пароль) подключает сама Windows.
    pub fn click(&mut self, i: usize) -> Click {
        let Some(n) = self.networks.get(i) else { return Click::Nothing };
        if n.connected {
            return Click::Nothing;
        }
        if !n.known {
            return Click::Networks;
        }
        if self.pending.is_none() {
            wifi::connect(&n.profile);
            self.pending = Some((n.ssid.clone(), Instant::now() + PENDING));
            self.next_poll = Instant::now() + Duration::from_millis(1200);
        }
        Click::Nothing
    }

    pub fn poll(&mut self, force: bool) -> bool {
        let now = Instant::now();
        if !force && now < self.next_poll {
            return false;
        }
        self.next_poll = now + POLL;
        let radio = wifi::radio_on();
        let fresh = if radio == Some(true) { wifi::networks() } else { Vec::new() };
        if let Some((ssid, until)) = &self.pending {
            if fresh.iter().any(|n| &n.ssid == ssid && n.connected) || now >= *until {
                self.pending = None;
            }
        }
        let changed = radio != self.radio || fresh != self.networks || self.pending.is_some();
        self.radio = radio;
        self.networks = fresh;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_height_adds_header_rows_and_footer() {
        assert_eq!(height(0), 244.0);
        assert_eq!(height(3), 144.0 + 22.0 + 132.0);
        assert_eq!(max_rows(body(3)), 3);
        assert_eq!(max_rows(0.0), 1);
    }

    #[test]
    fn bluetooth_rows_show_state_and_only_classic_devices_get_a_button() {
        let dev = |connected, le| Device { name: "x".into(), connected, address: 1, le };
        assert_eq!(bt_texts(&dev(true, false), None), ("Подключено", "Отключить"));
        assert_eq!(bt_texts(&dev(false, false), None), ("Не подключено", "Подключить"));
        assert_eq!(bt_texts(&dev(true, true), None), ("Подключено", ""));
        assert_eq!(bt_texts(&dev(false, false), Some(true)), ("Подключение…", ""));
        assert_eq!(bt_texts(&dev(true, false), Some(false)), ("Отключение…", ""));
    }

    #[test]
    fn wifi_rows_tell_connected_saved_and_new_networks_apart() {
        let net = |connected, known, secured| WifiNet { ssid: "n".into(), profile: "n".into(), signal: 70, secured, connected, known };
        assert_eq!(wifi_texts(&net(true, true, true), false), ("Подключено · 70%".to_string(), ""));
        assert_eq!(wifi_texts(&net(false, true, true), false), ("Сохранена · 70%".to_string(), "Подключить"));
        assert_eq!(wifi_texts(&net(false, false, true), false), ("Защищена · 70%".to_string(), "Войти"));
        assert_eq!(wifi_texts(&net(false, false, false), false), ("Открытая · 70%".to_string(), "Войти"));
        assert_eq!(wifi_texts(&net(false, true, true), true), ("Подключение…".to_string(), ""));
    }

    #[test]
    fn footer_counts_the_rows_that_did_not_fit() {
        assert_eq!(footer(0, "Параметры сети…", "в параметрах сети…"), "Параметры сети…");
        assert_eq!(footer(3, "Параметры сети…", "в параметрах сети…"), "Ещё 3 в параметрах сети…");
    }
}
