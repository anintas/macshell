//! Меню батареи: заряд, источник питания, оставшееся время, экономия заряда и процессы, сильнее всего
//! нагружающие процессор (у Windows нет открытого счётчика «энергии» процесса, поэтому берётся загрузка
//! процессора за короткий замер).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

/// Сколько длится замер загрузки процессора при открытии меню.
const SAMPLE: Duration = Duration::from_millis(300);
/// Процессы с меньшей загрузкой (в процентах всего процессора) в список не попадают.
const MIN_CPU_PERCENT: f64 = 0.5;
pub const MAX_HOGS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerInfo {
    /// Проценты; -1, если батареи нет или заряд неизвестен.
    pub percent: i32,
    /// Включено в сеть.
    pub ac: bool,
    pub charging: bool,
    /// Сколько работать от батареи, секунды; `None`, если Windows не знает.
    pub remaining_secs: Option<u32>,
    /// Включена экономия заряда.
    pub saver: bool,
}

/// Чистая часть чтения: поля `SYSTEM_POWER_STATUS` в `PowerInfo`.
pub fn info_from(ac_line: u8, flag: u8, percent: u8, life_secs: u32, system_flag: u8) -> PowerInfo {
    let has_battery = flag != 128 && flag != 255 && percent <= 100;
    PowerInfo {
        percent: if has_battery { percent as i32 } else { -1 },
        ac: ac_line == 1,
        charging: has_battery && flag & 8 != 0,
        // 0xFFFFFFFF это «неизвестно»; от сети время работы от батареи не считается.
        remaining_secs: (life_secs != u32::MAX && ac_line != 1).then_some(life_secs),
        saver: system_flag & 1 != 0,
    }
}

pub fn read() -> PowerInfo {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return info_from(255, 255, 255, u32::MAX, 0);
    }
    info_from(s.ACLineStatus, s.BatteryFlag, s.BatteryLifePercent, s.BatteryLifeTime, s.SystemStatusFlag)
}

/// «2 ч 15 мин», «45 мин», «меньше минуты».
pub fn format_duration(secs: u32) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    match (h, m) {
        (0, 0) => "меньше минуты".to_string(),
        (0, m) => format!("{m} мин"),
        (h, 0) => format!("{h} ч"),
        (h, m) => format!("{h} ч {m} мин"),
    }
}

/// Строки сведений для меню (без выключателей): заряд, источник, оставшееся время.
pub fn info_lines(info: &PowerInfo) -> Vec<String> {
    let mut lines = Vec::new();
    if info.percent >= 0 {
        let state = if info.charging { " · заряжается" } else { "" };
        lines.push(format!("Заряд: {} %{state}", info.percent));
    }
    lines.push(format!("Источник: {}", if info.ac { "сеть" } else { "батарея" }));
    if let Some(secs) = info.remaining_secs {
        lines.push(format!("Осталось: {}", format_duration(secs)));
    } else if !info.ac && info.percent >= 0 {
        lines.push("Осталось: считается…".to_string());
    }
    lines
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hog {
    pub name: String,
    /// Доля всего процессора, проценты.
    pub cpu: f64,
}

/// Процессорное время процесса (ядро + пользователь) в сотнях наносекунд.
type Cpu100ns = u64;

fn filetime(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// Имя программы без `.exe` и регистра для показа: `chrome.exe` даёт `chrome`.
fn short_name(exe: &str) -> String {
    exe.strip_suffix(".exe").or_else(|| exe.strip_suffix(".EXE")).unwrap_or(exe).to_string()
}

/// pid, имя файла и процессорное время всех процессов, к которым есть доступ.
fn snapshot_cpu() -> HashMap<u32, (String, Cpu100ns)> {
    let mut out = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let pid = entry.th32ProcessID;
            if pid != 0 {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
                    if GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user).is_ok() {
                        let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                        out.insert(pid, (String::from_utf16_lossy(&entry.szExeFile[..end]), filetime(kernel) + filetime(user)));
                    }
                    let _ = CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Чистая часть: из двух замеров и времени между ними (сотни наносекунд) выбирает самые нагружающие программы.
/// Процессы одной программы складываются; рисует панель, сама она (`own`) не в счёт.
pub fn rank_hogs(
    before: &HashMap<u32, (String, Cpu100ns)>,
    after: &HashMap<u32, (String, Cpu100ns)>,
    wall_100ns: u64,
    cpus: u32,
    own: &str,
    limit: usize,
) -> Vec<Hog> {
    if wall_100ns == 0 || cpus == 0 {
        return Vec::new();
    }
    let mut by_name: HashMap<String, u64> = HashMap::new();
    for (pid, (name, now)) in after {
        let Some((_, then)) = before.get(pid) else { continue };
        if name.eq_ignore_ascii_case(own) {
            continue;
        }
        *by_name.entry(short_name(name)).or_default() += now.saturating_sub(*then);
    }
    let capacity = wall_100ns as f64 * cpus as f64;
    let mut hogs: Vec<Hog> =
        by_name.into_iter().map(|(name, used)| Hog { name, cpu: used as f64 / capacity * 100.0 }).filter(|h| h.cpu >= MIN_CPU_PERCENT).collect();
    hogs.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(a.name.cmp(&b.name)));
    hogs.truncate(limit);
    hogs
}

/// Замеряет загрузку процессора за `SAMPLE` и возвращает самые нагружающие программы. Блокирует на это время.
pub fn top_cpu(limit: usize) -> Vec<Hog> {
    let before = snapshot_cpu();
    let started = Instant::now();
    std::thread::sleep(SAMPLE);
    let after = snapshot_cpu();
    let wall = (started.elapsed().as_nanos() / 100) as u64;
    let mut info = SYSTEM_INFO::default();
    unsafe { GetSystemInfo(&mut info) };
    rank_hogs(&before, &after, wall, info.dwNumberOfProcessors, "macshell.exe", limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_status_fields_become_readable_info() {
        let on_battery = info_from(0, 1, 64, 8100, 0);
        assert_eq!((on_battery.percent, on_battery.ac, on_battery.charging, on_battery.remaining_secs), (64, false, false, Some(8100)));
        let charging = info_from(1, 8, 80, u32::MAX, 1);
        assert!(charging.charging && charging.ac && charging.saver);
        assert_eq!(charging.remaining_secs, None);
        let desktop = info_from(1, 128, 255, u32::MAX, 0);
        assert_eq!(desktop.percent, -1);
        assert!(!desktop.charging);
        assert_eq!(info_from(0, 1, 50, u32::MAX, 0).remaining_secs, None, "unknown remaining time");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_duration(8100), "2 ч 15 мин");
        assert_eq!(format_duration(2700), "45 мин");
        assert_eq!(format_duration(3600), "1 ч");
        assert_eq!(format_duration(20), "меньше минуты");
    }

    #[test]
    fn info_lines_cover_charge_source_and_time() {
        let lines = info_lines(&info_from(0, 1, 64, 8100, 0));
        assert_eq!(lines, vec!["Заряд: 64 %", "Источник: батарея", "Осталось: 2 ч 15 мин"]);
        let lines = info_lines(&info_from(1, 8, 80, u32::MAX, 0));
        assert_eq!(lines, vec!["Заряд: 80 % · заряжается", "Источник: сеть"]);
        assert_eq!(info_lines(&info_from(0, 1, 50, u32::MAX, 0))[2], "Осталось: считается…");
        assert_eq!(info_lines(&info_from(1, 128, 255, u32::MAX, 0)), vec!["Источник: сеть"], "no battery: no charge line");
    }

    fn snap(rows: &[(u32, &str, u64)]) -> HashMap<u32, (String, Cpu100ns)> {
        rows.iter().map(|(pid, name, t)| (*pid, (name.to_string(), *t))).collect()
    }

    #[test]
    fn hogs_are_summed_per_program_ranked_and_filtered() {
        let before = snap(&[(1, "chrome.exe", 0), (2, "chrome.exe", 0), (3, "code.exe", 0), (4, "idle.exe", 0), (5, "macshell.exe", 0)]);
        // Замер 1 с на 4 процессорах: ёмкость 4 * 10^7 единиц.
        let after = snap(&[
            (1, "chrome.exe", 2_000_000),
            (2, "chrome.exe", 1_000_000),
            (3, "code.exe", 1_200_000),
            (4, "idle.exe", 1_000),
            (5, "macshell.exe", 9_000_000),
            (6, "new.exe", 9_000_000), // не было в первом замере: пропускается
        ]);
        let hogs = rank_hogs(&before, &after, 10_000_000, 4, "macshell.exe", 3);
        assert_eq!(hogs.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), vec!["chrome", "code"]);
        assert!((hogs[0].cpu - 7.5).abs() < 1e-9 && (hogs[1].cpu - 3.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_samples_give_no_hogs() {
        let a = snap(&[(1, "a.exe", 100)]);
        assert!(rank_hogs(&a, &a, 0, 4, "x", 3).is_empty());
        assert!(rank_hogs(&a, &a, 10, 0, "x", 3).is_empty());
        assert!(rank_hogs(&a, &a, 10_000_000, 4, "x", 3).is_empty(), "no time used");
    }

    #[test]
    fn real_sample_runs() {
        let _ = read();
        let hogs = top_cpu(MAX_HOGS);
        assert!(hogs.len() <= MAX_HOGS);
    }
}
