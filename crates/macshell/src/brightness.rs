//! Яркость внешнего монитора по DDC/CI (`dxva2`): ползунок в Центре управления. Каждый вызов к монитору занимает
//! десятки и сотни миллисекунд, поэтому всё делает один фоновый поток: он при старте читает яркость, а потом ждёт
//! новых значений и ставит последнее из них (промежуточные пропускаются). Встроенные экраны ноутбуков по DDC не
//! отвечают, у них ползунка нет: яркость там меняется в параметрах дисплея.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Once, OnceLock};
use std::thread::Thread;

use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    PHYSICAL_MONITOR, SetMonitorBrightness,
};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};

/// -1 ещё не известно, 0 монитор не поддерживает, 1 поддерживает.
static SUPPORTED: AtomicI32 = AtomicI32::new(-1);
/// Последняя известная яркость, доля от 0 до 1 (биты `f32`).
static LEVEL: AtomicU32 = AtomicU32::new(0);
/// Яркость, которую нужно поставить (проценты), или -1.
static TARGET: AtomicI32 = AtomicI32::new(-1);
static WORKER: OnceLock<Thread> = OnceLock::new();
static START: Once = Once::new();

/// Доля от 0 до 1 из значений монитора.
pub fn fraction(min: u32, cur: u32, max: u32) -> f32 {
    if max <= min {
        return 0.0;
    }
    (cur.clamp(min, max) - min) as f32 / (max - min) as f32
}

/// Значение для монитора из доли от 0 до 1.
pub fn raw(fraction: f32, min: u32, max: u32) -> u32 {
    if max <= min {
        return min;
    }
    min + (fraction.clamp(0.0, 1.0) * (max - min) as f32).round() as u32
}

pub fn supported() -> bool {
    SUPPORTED.load(Ordering::Relaxed) == 1
}

/// Последняя известная яркость, доля от 0 до 1.
pub fn level() -> f32 {
    f32::from_bits(LEVEL.load(Ordering::Relaxed))
}

/// Просит поставить яркость; поток применит её, когда монитор ответит.
pub fn set_level(fraction: f32) {
    let f = fraction.clamp(0.0, 1.0);
    LEVEL.store(f.to_bits(), Ordering::Relaxed);
    TARGET.store((f * 1000.0).round() as i32, Ordering::Relaxed);
    if let Some(t) = WORKER.get() {
        t.unpark();
    }
}

/// Запускает поток (один раз за процесс): читает яркость основного монитора и обслуживает запросы на смену.
pub fn start() {
    START.call_once(|| {
        let spawned = std::thread::Builder::new().name("brightness".into()).spawn(|| unsafe {
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut count = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count).is_err() || count == 0 {
                SUPPORTED.store(0, Ordering::Relaxed);
                return;
            }
            let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
            if GetPhysicalMonitorsFromHMONITOR(monitor, &mut monitors).is_err() {
                SUPPORTED.store(0, Ordering::Relaxed);
                return;
            }
            let handle = monitors[0].hPhysicalMonitor;
            let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
            if GetMonitorBrightness(handle, &mut min, &mut cur, &mut max) == 0 {
                SUPPORTED.store(0, Ordering::Relaxed);
                let _ = DestroyPhysicalMonitors(&monitors);
                return;
            }
            LEVEL.store(fraction(min, cur, max).to_bits(), Ordering::Relaxed);
            SUPPORTED.store(1, Ordering::Relaxed);
            loop {
                let target = TARGET.swap(-1, Ordering::Relaxed);
                if target < 0 {
                    std::thread::park();
                    continue;
                }
                SetMonitorBrightness(handle, raw(target as f32 / 1000.0, min, max));
            }
        });
        if let Ok(handle) = spawned {
            let _ = WORKER.set(handle.thread().clone());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_convert_both_ways() {
        assert_eq!(fraction(0, 50, 100), 0.5);
        assert_eq!(fraction(10, 10, 110), 0.0);
        assert_eq!(fraction(0, 200, 100), 1.0, "above the maximum is clamped");
        assert_eq!(fraction(5, 5, 5), 0.0, "degenerate range");
        assert_eq!(raw(0.5, 0, 100), 50);
        assert_eq!(raw(1.5, 10, 110), 110);
        assert_eq!(raw(-1.0, 10, 110), 10);
    }
}
