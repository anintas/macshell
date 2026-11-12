//! Пункты меню Apple: сон, перезагрузка, выключение, блокировка, выход из учётной записи, принудительное
//! завершение активного приложения. Опасные действия спрашивают подтверждение, как в macOS.

use std::os::windows::process::CommandExt;
use std::process::Command;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW,
};
use windows::core::PCWSTR;

use crate::tray::host::exe_of_pid;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Sleep,
    Restart,
    Shutdown,
    Lock,
    LogOff,
}

impl PowerAction {
    /// Вопрос перед действием, которое может потерять несохранённую работу; у сна и блокировки вопроса нет.
    pub fn question(self) -> Option<&'static str> {
        match self {
            PowerAction::Restart => Some("Перезагрузить компьютер сейчас?"),
            PowerAction::Shutdown => Some("Выключить компьютер сейчас?"),
            PowerAction::LogOff => Some("Выйти из учётной записи? Несохранённые данные могут пропасть."),
            PowerAction::Sleep | PowerAction::Lock => None,
        }
    }

    /// Аргументы `shutdown.exe`; `None` у действий без него.
    pub fn shutdown_args(self) -> Option<&'static [&'static str]> {
        match self {
            PowerAction::Restart => Some(&["/r", "/t", "0"]),
            PowerAction::Shutdown => Some(&["/s", "/t", "0"]),
            PowerAction::LogOff => Some(&["/l"]),
            PowerAction::Sleep | PowerAction::Lock => None,
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Окно с вопросом «Да/Нет». `true`, если выбрано «Да».
pub fn confirm(text: &str) -> bool {
    let (text, title) = (wide(text), wide("macshell"));
    unsafe { MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

/// Выполняет действие; те, что теряют работу, сначала спрашивают.
pub fn run(action: PowerAction) {
    if let Some(q) = action.question() {
        if !confirm(q) {
            return;
        }
    }
    match action {
        PowerAction::Sleep => unsafe {
            let _ = SetSuspendState(false, false, false);
        },
        PowerAction::Lock => {
            let _ = Command::new("rundll32.exe").args(["user32.dll,LockWorkStation"]).creation_flags(CREATE_NO_WINDOW).spawn();
        }
        other => {
            if let Some(args) = other.shutdown_args() {
                let _ = Command::new("shutdown.exe").args(args).creation_flags(CREATE_NO_WINDOW).spawn();
            }
        }
    }
}

/// Процесс активного окна: (pid, имя exe), кроме Проводника и самой оболочки: их завершать нельзя.
pub fn front_app() -> Option<(u32, String)> {
    let hwnd: HWND = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let exe = exe_of_pid(pid)?;
    let name = exe.rsplit(['\\', '/']).next().unwrap_or(&exe).to_string();
    killable(&name, pid, std::process::id()).then_some((pid, name))
}

/// Можно ли завершать процесс активного окна: не Проводник и не мы сами.
pub fn killable(exe_name: &str, pid: u32, own_pid: u32) -> bool {
    pid != 0 && pid != own_pid && !exe_name.eq_ignore_ascii_case("explorer.exe")
}

/// «Принудительно завершить» активное приложение после подтверждения. Возвращает имя завершённого.
pub fn force_quit_front() -> Option<String> {
    let (pid, name) = front_app()?;
    if !confirm(&format!("Принудительно завершить «{name}»? Несохранённые данные пропадут.")) {
        return None;
    }
    let ok = Command::new("taskkill.exe")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_work_losing_actions_ask_first() {
        assert!(PowerAction::Sleep.question().is_none());
        assert!(PowerAction::Lock.question().is_none());
        for a in [PowerAction::Restart, PowerAction::Shutdown, PowerAction::LogOff] {
            assert!(a.question().is_some(), "{a:?}");
            assert!(a.shutdown_args().is_some(), "{a:?} goes through shutdown.exe");
        }
        assert_eq!(PowerAction::Restart.shutdown_args(), Some(&["/r", "/t", "0"][..]));
    }

    #[test]
    fn explorer_ourselves_and_pid_zero_are_never_killed() {
        assert!(!killable("explorer.exe", 100, 1));
        assert!(!killable("Explorer.EXE", 100, 1));
        assert!(!killable("notepad.exe", 1, 1));
        assert!(!killable("notepad.exe", 0, 1));
        assert!(killable("notepad.exe", 100, 1));
    }
}
