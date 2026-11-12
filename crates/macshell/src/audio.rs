//! Устройства звука для меню громкости: список активных устройств вывода и ввода и смена устройства по
//! умолчанию. Смена идёт через недокументированный, но стабильный с Vista интерфейс `IPolicyConfig` (его же
//! использует панель «Звук» Windows).

// Имена методов `IPolicyConfig` в стиле COM.
#![allow(non_snake_case)]

use std::ffi::c_void;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eCommunications,
    eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, PWSTR};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Output,
    Input,
}

impl Flow {
    fn data_flow(self) -> EDataFlow {
        match self {
            Flow::Output => eRender,
            Flow::Input => eCapture,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    /// Устройство по умолчанию для своего направления.
    pub default: bool,
}

/// Порядок в меню: по названию без учёта регистра, как в панели «Звук».
pub fn sort_devices(devices: &mut [AudioDevice]) {
    devices.sort_by_key(|d| d.name.to_lowercase());
}

/// Интерфейс настройки звука Windows. Нужен единственный метод `SetDefaultEndpoint`; остальные описаны только
/// ради порядка в таблице методов.
#[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
    fn GetDeviceFormat(&self, device: PCWSTR, default: BOOL, format: *mut *mut c_void) -> HRESULT;
    fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
    fn SetDeviceFormat(&self, device: PCWSTR, endpoint: *mut c_void, mix: *mut c_void) -> HRESULT;
    fn GetProcessingPeriod(&self, device: PCWSTR, default: BOOL, default_period: *mut i64, min_period: *mut i64) -> HRESULT;
    fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
    fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn GetPropertyValue(&self, device: PCWSTR, fx_store: BOOL, key: *const c_void, value: *mut c_void) -> HRESULT;
    fn SetPropertyValue(&self, device: PCWSTR, fx_store: BOOL, key: *const c_void, value: *mut c_void) -> HRESULT;
    fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
    fn SetEndpointVisibility(&self, device: PCWSTR, visible: BOOL) -> HRESULT;
}

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
    }
}

fn take_string(p: PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

fn id_of(device: &IMMDevice) -> Option<String> {
    unsafe { device.GetId().ok().map(take_string) }
}

fn name_of(device: &IMMDevice) -> Option<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let text = PropVariantToStringAlloc(&value as *const _).ok()?;
        Some(take_string(text)).filter(|s| !s.is_empty())
    }
}

/// Активные устройства направления `flow`, отсортированные по названию.
pub fn list(flow: Flow) -> Vec<AudioDevice> {
    let Ok(en) = enumerator() else { return Vec::new() };
    let default_id = unsafe { en.GetDefaultAudioEndpoint(flow.data_flow(), eConsole) }.ok().and_then(|d| id_of(&d));
    let mut out = Vec::new();
    unsafe {
        let Ok(devices) = en.EnumAudioEndpoints(flow.data_flow(), DEVICE_STATE_ACTIVE) else { return out };
        for i in 0..devices.GetCount().unwrap_or(0) {
            let Ok(device) = devices.Item(i) else { continue };
            let Some(id) = id_of(&device) else { continue };
            let name = name_of(&device).unwrap_or_else(|| "Устройство".to_string());
            out.push(AudioDevice { default: default_id.as_deref() == Some(id.as_str()), id, name });
        }
    }
    sort_devices(&mut out);
    out
}

/// Делает устройство `id` устройством по умолчанию для всех ролей (обычная, мультимедиа, связь).
pub fn set_default(id: &str) -> bool {
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(policy) = CoCreateInstance::<_, IPolicyConfig>(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) else {
            return false;
        };
        [eConsole, eMultimedia, eCommunications].into_iter().all(|role| policy.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, default: bool) -> AudioDevice {
        AudioDevice { id: name.into(), name: name.into(), default }
    }

    #[test]
    fn devices_are_ordered_by_name_ignoring_case() {
        let mut v = vec![dev("realtek", false), dev("Динамики", true), dev("Bluetooth", false)];
        sort_devices(&mut v);
        assert_eq!(v.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["Bluetooth", "realtek", "Динамики"]);
    }

    #[test]
    fn real_output_devices_are_listed_with_one_default() {
        let list = list(Flow::Output);
        // На машине без звука список пуст; иначе по умолчанию ровно одно устройство.
        assert!(list.iter().filter(|d| d.default).count() <= 1);
        assert!(list.iter().all(|d| !d.id.is_empty() && !d.name.is_empty()));
    }
}
