//! Значки состояния верхней полосы: сеть, громкость, батарея. Обновление по событиям системы.
//! Батарея приходит окну сама (`WM_POWERBROADCAST`), сеть и громкость сообщают через `watch`.

use std::ffi::c_void;
use std::sync::Once;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender,
};
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    GetAdaptersAddresses, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH, MIB_IPINTERFACE_ROW,
    MIB_NOTIFICATION_TYPE,
    NotifyIpInterfaceChange,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::AF_UNSPEC;
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::implement;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// 0 нет сети, 1 Wi-Fi, 2 проводная.
    pub wifi: i32,
    /// 0 выключен или ноль, 1 тихо, 2 громко.
    pub volume: i32,
    /// -1 нет батареи, иначе проценты.
    pub battery: i32,
    pub charging: bool,
    /// 0 нет радио, 1 выключен, 2 включён (см. `bluetooth`).
    pub bluetooth: i32,
    /// Микрофон занят какой-то программой (оранжевая точка).
    pub mic: bool,
    /// Камера занята (зелёная точка).
    pub camera: bool,
    /// Поднято VPN-соединение.
    pub vpn: bool,
    /// Включено «Не беспокоить».
    pub focus: bool,
}

/// Адаптер похож на VPN: соединение PPP (встроенный VPN Windows) или туннель известной программы.
pub fn looks_like_vpn(if_type: u32, description: &str) -> bool {
    if if_type == IF_TYPE_PPP {
        return true;
    }
    let d = description.to_lowercase();
    ["wireguard", "wintun", "openvpn", "tap-windows", "nordlynx", "tailscale", "vpn", "anyconnect", "zerotier"].iter().any(|k| d.contains(k))
}

/// Проценты и признак зарядки для значка. `255` значит «неизвестно» (так отвечает Windows).
pub fn battery_from(percent: u8, ac_online: bool, has_battery: bool) -> (i32, bool) {
    if !has_battery || percent > 100 {
        return (-1, false);
    }
    (percent as i32, ac_online && percent < 100)
}

fn battery_state() -> (i32, bool) {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return (-1, false);
    }
    // BatteryFlag: 128 нет батареи, 255 неизвестно.
    let has_battery = s.BatteryFlag != 128 && s.BatteryFlag != 255;
    battery_from(s.BatteryLifePercent, s.ACLineStatus == 1, has_battery)
}

const IF_TYPE_PPP: u32 = 23;

/// Сеть с маршрутом по умолчанию (проводная важнее Wi-Fi) и признак поднятого VPN.
fn network_state() -> (i32, bool) {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    // u64, чтобы буфер был выровнен под структуры адаптеров.
    let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
    loop {
        let ret = unsafe {
            GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size)
        };
        if ret == ERROR_BUFFER_OVERFLOW.0 {
            buf = vec![0; (size as usize).div_ceil(8)];
        } else if ret == 0 {
            break;
        } else {
            return (0, false);
        }
    }
    let (mut wifi, mut wired, mut vpn) = (false, false, false);
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !p.is_null() {
        let a = unsafe { &*p };
        if a.OperStatus == IfOperStatusUp {
            if !a.FirstGatewayAddress.is_null() {
                match a.IfType {
                    IF_TYPE_IEEE80211 => wifi = true,
                    IF_TYPE_ETHERNET_CSMACD => wired = true,
                    _ => {}
                }
            }
            if !vpn && !a.FirstUnicastAddress.is_null() {
                let description = if a.Description.is_null() { String::new() } else { unsafe { a.Description.to_string() }.unwrap_or_default() };
                vpn = looks_like_vpn(a.IfType, &description);
            }
        }
        p = a.Next;
    }
    (if wired { 2 } else if wifi { 1 } else { 0 }, vpn)
}

fn endpoint() -> windows::core::Result<IAudioEndpointVolume> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
        device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
    }
}

fn volume_state() -> i32 {
    let Ok(ep) = endpoint() else { return 2 };
    unsafe {
        let muted = ep.GetMute().map(|m| m.as_bool()).unwrap_or(false);
        let level = ep.GetMasterVolumeLevelScalar().unwrap_or(1.0);
        if muted || level < 0.01 {
            0
        } else if level < 0.5 {
            1
        } else {
            2
        }
    }
}

pub fn read() -> Status {
    use crate::privacy::{Device, active};
    let (battery, charging) = battery_state();
    let (wifi, vpn) = network_state();
    Status {
        wifi,
        volume: volume_state(),
        battery,
        charging,
        bluetooth: crate::bluetooth::state(),
        mic: active(Device::Microphone),
        camera: active(Device::Camera),
        vpn,
        focus: crate::focus::is_on(),
    }
}

/// Громкость системного устройства вывода для окна с ползунком. Устройство берётся один раз при открытии окна.
pub struct VolumeControl(IAudioEndpointVolume);

impl VolumeControl {
    pub fn open() -> Option<VolumeControl> {
        endpoint().ok().map(VolumeControl)
    }

    /// Громкость от 0 до 1.
    pub fn level(&self) -> f32 {
        unsafe { self.0.GetMasterVolumeLevelScalar() }.unwrap_or(0.0).clamp(0.0, 1.0)
    }

    pub fn muted(&self) -> bool {
        unsafe { self.0.GetMute() }.map(|m| m.as_bool()).unwrap_or(false)
    }

    /// Ставит громкость; ненулевая громкость снимает «без звука», как в системе.
    pub fn set_level(&self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        unsafe {
            let _ = self.0.SetMasterVolumeLevelScalar(level, std::ptr::null());
            if level > 0.0 && self.muted() {
                let _ = self.0.SetMute(false, std::ptr::null());
            }
        }
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe {
            let _ = self.0.SetMute(muted, std::ptr::null());
        }
    }
}

/// Будит полосу, как это делают сеть и громкость: она перечитает состояние.
pub fn notify() {
    post();
}

static NOTIFY_HWND: AtomicIsize = AtomicIsize::new(0);
static NOTIFY_MSG: AtomicU32 = AtomicU32::new(0);

fn post() {
    let target = NOTIFY_HWND.load(Ordering::Relaxed);
    if target != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(target as *mut _)), NOTIFY_MSG.load(Ordering::Relaxed), WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn on_ip_change(_ctx: *const c_void, _row: *const MIB_IPINTERFACE_ROW, _kind: MIB_NOTIFICATION_TYPE) {
    post();
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback;

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        post();
        Ok(())
    }
}

/// Живёт до конца процесса: подписки не снимаются, система сама чистит их при выходе.
struct Subscriptions {
    _endpoint: Option<IAudioEndpointVolume>,
    _callback: Option<IAudioEndpointVolumeCallback>,
}

/// Окну `hwnd` приходит сообщение `msg` при смене сети или громкости. Вызывать из потока с циклом сообщений.
pub fn watch(hwnd: isize, msg: u32) {
    NOTIFY_HWND.store(hwnd, Ordering::Relaxed);
    NOTIFY_MSG.store(msg, Ordering::Relaxed);
    // Подписки ставятся один раз: при пересборке полосы меняется только адресат сообщения.
    SUBSCRIBE.call_once(|| {
        unsafe {
            let mut handle = HANDLE::default();
            let _ = NotifyIpInterfaceChange(AF_UNSPEC, Some(on_ip_change), None, false, &mut handle);
        }
        crate::privacy::watch(post);
        crate::focus::watch(post);
        let mut keep = Subscriptions { _endpoint: None, _callback: None };
        if let Ok(ep) = endpoint() {
            let cb: IAudioEndpointVolumeCallback = VolumeCallback.into();
            if unsafe { ep.RegisterControlChangeNotify(&cb) }.is_ok() {
                keep._endpoint = Some(ep);
                keep._callback = Some(cb);
            }
        }
        Box::leak(Box::new(keep));
    });
}

static SUBSCRIBE: Once = Once::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn_adapters_are_recognised() {
        assert!(looks_like_vpn(23, "WAN Miniport (PPTP)"));
        assert!(looks_like_vpn(53, "WireGuard Tunnel"));
        assert!(looks_like_vpn(6, "TAP-Windows Adapter V9"));
        assert!(!looks_like_vpn(6, "Realtek PCIe GbE Family Controller"));
        assert!(!looks_like_vpn(71, "Intel(R) Wi-Fi 6 AX201"));
    }

    #[test]
    fn no_battery_hides_icon() {
        assert_eq!(battery_from(0, true, false), (-1, false));
    }

    #[test]
    fn charging_flag() {
        assert_eq!(battery_from(80, true, true), (80, true));
    }

    #[test]
    fn full_on_ac_is_not_charging() {
        assert_eq!(battery_from(100, true, true), (100, false));
    }

    #[test]
    fn unknown_percent_hides_icon() {
        assert_eq!(battery_from(255, false, true), (-1, false));
    }

    #[test]
    #[ignore]
    fn read_prints_plausible_values() {
        let s = read();
        println!("{s:?}");
        assert!((0..=2).contains(&s.wifi) && (0..=2).contains(&s.volume) && (-1..=100).contains(&s.battery));
    }
}
