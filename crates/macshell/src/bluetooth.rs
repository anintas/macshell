//! Bluetooth для верхней полосы: есть ли радио и включено ли оно. Читается через `Windows.Devices.Radios`, смена
//! состояния приходит событием `StateChanged`. Поток с подпиской один и спит между событиями.

use std::sync::Once;
use std::sync::atomic::{AtomicI32, Ordering};

use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Foundation::TypedEventHandler;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::IInspectable;

/// 0 нет радио, 1 выключено, 2 включено.
static STATE: AtomicI32 = AtomicI32::new(0);
static START: Once = Once::new();

pub const NONE: i32 = 0;
pub const OFF: i32 = 1;
pub const ON: i32 = 2;

/// Последнее известное состояние (без обращения к системе).
pub fn state() -> i32 {
    STATE.load(Ordering::Relaxed)
}

/// Сводка по всем радио: включено хотя бы одно, значит включено.
pub fn summarize(radios_on: &[bool]) -> i32 {
    if radios_on.is_empty() {
        NONE
    } else if radios_on.iter().any(|on| *on) {
        ON
    } else {
        OFF
    }
}

fn bluetooth_radios() -> windows::core::Result<Vec<Radio>> {
    let mut out = Vec::new();
    for radio in Radio::GetRadiosAsync()?.join()? {
        if radio.Kind()? == RadioKind::Bluetooth {
            out.push(radio);
        }
    }
    Ok(out)
}

fn compute() -> i32 {
    let radios = bluetooth_radios().unwrap_or_default();
    let on: Vec<bool> = radios.iter().map(|r| r.State() == Ok(RadioState::On)).collect();
    summarize(&on)
}

fn publish(value: i32) {
    if STATE.swap(value, Ordering::Relaxed) != value {
        crate::status::notify(); // полоса перечитает состояние и перерисует значок
    }
}

/// Запускает наблюдение (один раз за процесс). Изменения сообщаются тому же окну, что и сеть с громкостью.
pub fn watch() {
    START.call_once(|| {
        let _ = std::thread::Builder::new().name("bluetooth".into()).spawn(|| {
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
            }
            publish(compute());
            let Ok(radios) = bluetooth_radios() else { return };
            let handler = TypedEventHandler::<Radio, IInspectable>::new(|_, _| {
                publish(compute());
                Ok(())
            });
            for radio in &radios {
                let _ = radio.StateChanged(&handler);
            }
            // Радио и подписки живут, пока живёт поток.
            loop {
                std::thread::park();
            }
        });
    });
}

/// Включает или выключает Bluetooth (в фоне: система отвечает не сразу).
pub fn set_enabled(on: bool) {
    let _ = std::thread::Builder::new().name("bluetooth-set".into()).spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let _ = Radio::RequestAccessAsync().and_then(|a| a.join());
        let target = if on { RadioState::On } else { RadioState::Off };
        for radio in bluetooth_radios().unwrap_or_default() {
            let _ = radio.SetStateAsync(target).and_then(|a| a.join());
        }
        publish(compute());
    });
}

/// Сопряжённое устройство для меню Bluetooth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub connected: bool,
    /// Адрес устройства; подключать и отключать можно только классические (`le == false`).
    pub address: u64,
    /// Устройство Bluetooth LE: показывается, но Windows не даёт управлять его соединением из программы.
    pub le: bool,
}

/// Порядок в меню: подключённые сверху, дальше по имени; пустые имена и повторы убираются.
pub fn arrange(mut devices: Vec<Device>) -> Vec<Device> {
    devices.retain(|d| !d.name.trim().is_empty());
    devices.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let mut seen = std::collections::HashSet::new();
    devices.retain(|d| seen.insert(d.name.to_lowercase()));
    devices
}

static LE_DEVICES: std::sync::Mutex<Vec<Device>> = std::sync::Mutex::new(Vec::new());

/// Устройства BLE: читаются в фоне (по каждому нужен запрос к системе), меню берёт то, что нашлось в прошлый раз.
fn refresh_le() {
    let _ = std::thread::Builder::new().name("bluetooth-le".into()).spawn(|| {
        use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothLEDevice};
        use windows::Devices::Enumeration::DeviceInformation;
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let found = (|| -> windows::core::Result<Vec<Device>> {
            let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?;
            let mut out = Vec::new();
            for info in DeviceInformation::FindAllAsyncAqsFilter(&selector)?.join()? {
                let connected = BluetoothLEDevice::FromIdAsync(&info.Id()?)?
                    .join()
                    .and_then(|d| d.ConnectionStatus())
                    .map(|s| s == BluetoothConnectionStatus::Connected)
                    .unwrap_or(false);
                out.push(Device { name: info.Name()?.to_string(), connected, address: 0, le: true });
            }
            Ok(out)
        })();
        if let Ok(list) = found {
            *LE_DEVICES.lock().unwrap() = list;
        }
    });
}

/// Сопряжённые классические устройства (имя, подключено ли) из кэша системы: быстро, радио не опрашивается.
fn classic_devices() -> Vec<Device> {
    use windows::Win32::Devices::Bluetooth::{
        BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS, BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice,
    };
    let mut out = Vec::new();
    let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
        dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
        fReturnAuthenticated: true.into(),
        fReturnRemembered: true.into(),
        fReturnUnknown: false.into(),
        fReturnConnected: true.into(),
        fIssueInquiry: false.into(),
        cTimeoutMultiplier: 0,
        hRadio: Default::default(),
    };
    let mut info = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
    unsafe {
        let Ok(find) = BluetoothFindFirstDevice(&params, &mut info) else { return out };
        loop {
            let end = info.szName.iter().position(|&c| c == 0).unwrap_or(info.szName.len());
            out.push(Device {
                name: String::from_utf16_lossy(&info.szName[..end]),
                connected: info.fConnected.as_bool(),
                address: info.Address.Anonymous.ullLong,
                le: false,
            });
            if BluetoothFindNextDevice(find, &mut info).is_err() {
                break;
            }
        }
        let _ = BluetoothFindDeviceClose(find);
    }
    out
}

/// Сопряжённые устройства для меню: классические сразу, BLE из прошлого чтения (новое запускается в фоне).
pub fn devices() -> Vec<Device> {
    refresh_le();
    let mut all = classic_devices();
    all.extend(LE_DEVICES.lock().unwrap().iter().cloned());
    arrange(all)
}

/// Как `devices`, но BLE берётся из кэша без нового запроса: для опроса раз в пару секунд.
pub fn devices_cached() -> Vec<Device> {
    let mut all = classic_devices();
    all.extend(LE_DEVICES.lock().unwrap().iter().cloned());
    arrange(all)
}

/// Что это за устройство по имени: 0 общее, 1 наушники, 2 колонка, 3 мышь, 4 клавиатура, 5 геймпад, 6 телефон, 7 часы.
pub fn kind_of(name: &str) -> i32 {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["controller", "gamepad", "dualsense", "dualshock", "xbox", "геймпад", "джойстик"]) {
        5
    } else if has(&["watch", "band", "часы", "браслет"]) {
        7
    } else if has(&["headphone", "headset", "earbud", "buds", "airpods", "pods", "wh-", "wf-", "jabra", "наушник", "гарнитур"]) {
        1
    } else if has(&["speaker", "soundbar", "boombox", "jbl", "колонк", "echo", "homepod"]) {
        2
    } else if has(&["mouse", "мыш", "mx anywhere", "mx master"]) {
        3
    } else if has(&["keyboard", "клавиатур", "mx keys", "k380"]) {
        4
    } else if has(&["iphone", "phone", "galaxy", "pixel", "redmi", "xiaomi", "poco", "телефон", "oneplus", "honor", "huawei"]) {
        6
    } else {
        0
    }
}

/// Подключает или отключает классическое устройство: включает или выключает его службы звука и гарнитуры. В фоне,
/// система отвечает секунды.
pub fn set_connected(address: u64, on: bool) {
    use windows::Win32::Devices::Bluetooth::{BLUETOOTH_DEVICE_INFO, BLUETOOTH_ADDRESS, BLUETOOTH_ADDRESS_0, BluetoothGetDeviceInfo, BluetoothSetServiceState};
    use windows::core::GUID;
    // A2DP sink, гарнитура, hands-free, AVRCP: службы, из которых система собирает подключение наушников и колонок.
    const SERVICES: [GUID; 4] = [
        GUID::from_u128(0x0000110b_0000_1000_8000_00805f9b34fb),
        GUID::from_u128(0x00001108_0000_1000_8000_00805f9b34fb),
        GUID::from_u128(0x0000111e_0000_1000_8000_00805f9b34fb),
        GUID::from_u128(0x0000110e_0000_1000_8000_00805f9b34fb),
    ];
    let _ = std::thread::Builder::new().name("bluetooth-link".into()).spawn(move || unsafe {
        let mut info = BLUETOOTH_DEVICE_INFO {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
            Address: BLUETOOTH_ADDRESS { Anonymous: BLUETOOTH_ADDRESS_0 { ullLong: address } },
            ..Default::default()
        };
        if BluetoothGetDeviceInfo(None, &mut info) != 0 {
            return;
        }
        for guid in &SERVICES {
            if on {
                // Выключение и включение службы заставляет систему установить соединение заново.
                let _ = BluetoothSetServiceState(None, &info, guid, 0);
                let _ = BluetoothSetServiceState(None, &info, guid, 1);
            } else {
                let _ = BluetoothSetServiceState(None, &info, guid, 0);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, connected: bool) -> Device {
        Device { name: name.into(), connected, address: 1, le: false }
    }

    #[test]
    fn devices_are_arranged_connected_first_then_by_name_without_blanks_or_repeats() {
        let list = arrange(vec![dev("Мышь", false), dev("  ", true), dev("наушники", true), dev("Клавиатура", false), dev("МЫШЬ", true)]);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        // Повтор «Мышь» убирается вместе с неподключённой копией: остаётся подключённая.
        assert_eq!(names, vec!["МЫШЬ", "наушники", "Клавиатура"]);
    }

    #[test]
    fn real_device_list_does_not_panic() {
        let _ = classic_devices();
    }

    #[test]
    fn no_radios_means_none() {
        assert_eq!(summarize(&[]), NONE);
    }

    #[test]
    fn any_radio_on_means_on() {
        assert_eq!(summarize(&[false, true]), ON);
        assert_eq!(summarize(&[true]), ON);
    }

    #[test]
    fn all_off_means_off() {
        assert_eq!(summarize(&[false, false]), OFF);
    }

    #[test]
    #[ignore]
    fn real_state_is_readable() {
        println!("bluetooth: {}", compute());
    }
}

#[cfg(test)]
mod kind_tests {
    use super::kind_of;

    #[test]
    fn names_map_to_device_kinds() {
        assert_eq!(kind_of("WH-1000XM5"), 1);
        assert_eq!(kind_of("AirPods Pro"), 1);
        assert_eq!(kind_of("JBL Flip 6"), 2);
        assert_eq!(kind_of("Logitech MX Master 3"), 3);
        assert_eq!(kind_of("Беспроводная клавиатура"), 4);
        assert_eq!(kind_of("Xbox Wireless Controller"), 5);
        assert_eq!(kind_of("iPhone Виктора"), 6);
        assert_eq!(kind_of("Mi Smart Band 8"), 7);
        assert_eq!(kind_of("Unknown 42"), 0);
    }
}
