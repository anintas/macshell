//! Wi-Fi для меню в строке меню: список сетей рядом (`wlanapi`), подключение к уже известной сети и
//! выключатель радио (`Windows.Devices.Radios`). К новой защищённой сети пароль вводит сама Windows: для неё
//! меню открывает её сетевую панель.

use std::ffi::c_void;

use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_AVAILABLE_NETWORK_CONNECTED, WLAN_AVAILABLE_NETWORK_HAS_PROFILE, WLAN_AVAILABLE_NETWORK_LIST, WLAN_CONNECTION_PARAMETERS,
    WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanConnect, WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList,
    WlanOpenHandle, WlanScan, dot11_BSS_type_any, wlan_connection_mode_profile,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{GUID, PCWSTR};

/// Сколько сетей показывает меню.
pub const MAX_NETWORKS: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiNet {
    pub ssid: String,
    /// Имя профиля Windows (у известных сетей); по нему идёт подключение.
    pub profile: String,
    /// Качество сигнала, 0..100.
    pub signal: u32,
    pub secured: bool,
    pub connected: bool,
    /// Для сети в Windows есть сохранённый профиль (пароль уже вводили).
    pub known: bool,
}

/// Одна запись на название сети (самый сильный сигнал, признаки объединяются), без скрытых сетей. Порядок:
/// подключённая, затем известные, затем по силе сигнала; не больше `MAX_NETWORKS`.
pub fn merge_networks(raw: Vec<WifiNet>) -> Vec<WifiNet> {
    let mut out: Vec<WifiNet> = Vec::new();
    for net in raw.into_iter().filter(|n| !n.ssid.is_empty()) {
        match out.iter_mut().find(|n| n.ssid == net.ssid) {
            Some(have) => {
                have.connected |= net.connected;
                have.known |= net.known;
                if have.profile.is_empty() {
                    have.profile = net.profile.clone();
                }
                have.signal = have.signal.max(net.signal);
            }
            None => out.push(net),
        }
    }
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.known.cmp(&a.known)).then(b.signal.cmp(&a.signal)).then(a.ssid.cmp(&b.ssid)));
    out.truncate(MAX_NETWORKS);
    out
}

/// Открытый клиент Wlan; закрывается сам.
struct Client(HANDLE);

impl Client {
    fn open() -> Option<Client> {
        let mut negotiated = 0u32;
        let mut handle = HANDLE::default();
        let code = unsafe { WlanOpenHandle(2, None, &mut negotiated, &mut handle) };
        (code == 0).then_some(Client(handle))
    }

    /// Идентификаторы адаптеров Wi-Fi.
    fn interfaces(&self) -> Vec<GUID> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let mut out = Vec::new();
        unsafe {
            if WlanEnumInterfaces(self.0, None, &mut list) != 0 || list.is_null() {
                return out;
            }
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize);
            out.extend(items.iter().map(|i| i.InterfaceGuid));
            WlanFreeMemory(list as *const c_void);
        }
        out
    }

    fn networks_of(&self, guid: &GUID) -> Vec<WifiNet> {
        let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
        let mut out = Vec::new();
        unsafe {
            // Запрос на свежее сканирование: следующее открытие меню увидит новые сети.
            let _ = WlanScan(self.0, guid, None, None, None);
            if WlanGetAvailableNetworkList(self.0, guid, 0, None, &mut list) != 0 || list.is_null() {
                return out;
            }
            let items = std::slice::from_raw_parts((*list).Network.as_ptr(), (*list).dwNumberOfItems as usize);
            for n in items {
                let len = (n.dot11Ssid.uSSIDLength as usize).min(32);
                let profile_len = n.strProfileName.iter().position(|&c| c == 0).unwrap_or(n.strProfileName.len());
                out.push(WifiNet {
                    ssid: String::from_utf8_lossy(&n.dot11Ssid.ucSSID[..len]).into_owned(),
                    profile: String::from_utf16_lossy(&n.strProfileName[..profile_len]),
                    signal: n.wlanSignalQuality.min(100),
                    secured: n.bSecurityEnabled.as_bool(),
                    connected: n.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                    known: n.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0,
                });
            }
            WlanFreeMemory(list as *const c_void);
        }
        out
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            let _ = WlanCloseHandle(self.0, None);
        }
    }
}

/// Сети в зоне приёмa; пусто, если адаптера Wi-Fi нет.
pub fn networks() -> Vec<WifiNet> {
    let Some(client) = Client::open() else { return Vec::new() };
    let raw = client.interfaces().iter().flat_map(|g| client.networks_of(g)).collect();
    merge_networks(raw)
}

/// Подключает к известной сети по имени профиля. `false`, если не вышло.
pub fn connect(profile: &str) -> bool {
    let Some(client) = Client::open() else { return false };
    let Some(guid) = client.interfaces().into_iter().next() else { return false };
    let wide: Vec<u16> = profile.encode_utf16().chain(std::iter::once(0)).collect();
    let params = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(wide.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_any,
        dwFlags: 0,
    };
    unsafe { WlanConnect(client.0, &guid, &params, None) == 0 }
}

fn wifi_radios() -> windows::core::Result<Vec<Radio>> {
    let mut out = Vec::new();
    for radio in Radio::GetRadiosAsync()?.join()? {
        if radio.Kind()? == RadioKind::WiFi {
            out.push(radio);
        }
    }
    Ok(out)
}

/// Радио Wi-Fi включено; `None`, если радио нет. Блокирует на время запроса к системе.
pub fn radio_on() -> Option<bool> {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let radios = wifi_radios().ok()?;
    (!radios.is_empty()).then(|| radios.iter().any(|r| r.State() == Ok(RadioState::On)))
}

/// Включает или выключает Wi-Fi (в фоне: система отвечает не сразу).
pub fn set_enabled(on: bool) {
    let _ = std::thread::Builder::new().name("wifi-set".into()).spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let _ = Radio::RequestAccessAsync().and_then(|a| a.join());
        let target = if on { RadioState::On } else { RadioState::Off };
        for radio in wifi_radios().unwrap_or_default() {
            let _ = radio.SetStateAsync(target).and_then(|a| a.join());
        }
        crate::status::notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ssid: &str, signal: u32, connected: bool, known: bool) -> WifiNet {
        WifiNet { ssid: ssid.into(), profile: if known { ssid.into() } else { String::new() }, signal, secured: true, connected, known }
    }

    #[test]
    fn duplicates_merge_keeping_the_strongest_signal_and_all_flags() {
        let merged = merge_networks(vec![net("Home", 40, false, false), net("Home", 80, true, true), net("", 90, false, false)]);
        assert_eq!(merged.len(), 1, "hidden network (empty name) is dropped");
        assert_eq!((merged[0].signal, merged[0].connected, merged[0].known), (80, true, true));
        assert_eq!(merged[0].profile, "Home");
    }

    #[test]
    fn connected_first_then_known_then_by_signal() {
        let merged = merge_networks(vec![
            net("Strong", 90, false, false),
            net("Cafe", 50, false, true),
            net("Office", 30, true, true),
            net("Weak", 10, false, false),
        ]);
        let names: Vec<&str> = merged.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, vec!["Office", "Cafe", "Strong", "Weak"]);
    }

    #[test]
    fn list_is_capped() {
        let many: Vec<WifiNet> = (0..30).map(|i| net(&format!("n{i:02}"), i, false, false)).collect();
        assert_eq!(merge_networks(many).len(), MAX_NETWORKS);
    }

    #[test]
    #[ignore]
    fn real_scan_prints_networks() {
        println!("{:?}", networks());
        println!("radio: {:?}", radio_on());
    }
}
