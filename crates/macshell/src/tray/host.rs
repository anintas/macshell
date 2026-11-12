//! Хост трея: окно класса `Shell_TrayWnd`, которое `FindWindow` находит раньше окна Проводника.
//! Значки приходят как `WM_COPYDATA` (`dwData = 1`); всё остальное, что адресовано `Shell_TrayWnd`
//! (в первую очередь сообщения `AppBar`), пересылается настоящему окну Проводника.

use std::cell::{Cell, RefCell};

use anyhow::{Result, bail};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, GetWindowRect, SWP_NOZORDER, DefWindowProcW, DestroyWindow, FindWindowExW, GetCursorPos,
    GetWindowThreadProcessId, HWND_BROADCAST, HWND_MESSAGE, HWND_TOPMOST, IsWindow, PostMessageW, RegisterClassW, RegisterWindowMessageW,
    SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SendMessageTimeoutW, SetWindowPos, WM_COMMAND, WM_COPYDATA,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_USER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::proto::{COPYDATA_TRAY, parse_copydata};
use super::state::{TrayIcon, TrayState};

const ASFW_ANY: u32 = u32::MAX;
const NIN_SELECT: u32 = 0x0400;
const WM_CONTEXTMENU: u32 = 0x007B;
const NOTIFYICON_VERSION_4: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

thread_local! {
    static STATE: RefCell<TrayState> = RefCell::new(TrayState::default());
    /// Путь к exe по окну-владельцу значка.
    static EXE_CACHE: RefCell<std::collections::HashMap<isize, Option<String>>> = RefCell::new(Default::default());
    /// Наше окно, чтобы найти настоящий `Shell_TrayWnd` позади него.
    static OURS: Cell<isize> = const { Cell::new(0) };
    /// Окно и сообщение, которым сообщаем о смене списка значков.
    static NOTIFY: Cell<(isize, u32)> = const { Cell::new((0, 0)) };
}

/// Окно Проводника: первый `Shell_TrayWnd` в Z-порядке после нашего.
fn explorer_tray() -> Option<HWND> {
    let ours = OURS.get();
    unsafe {
        let h = FindWindowExW(None, Some(HWND(ours as *mut _)), w!("Shell_TrayWnd"), PCWSTR::null()).ok()?;
        (!h.0.is_null() && h.0 as isize != ours).then_some(h)
    }
}

/// Выполняет `f`, пока окно хоста стоит позади окна Проводника. Нужно сообщениям `AppBar` (`ABM_QUERYPOS`,
/// `ABM_SETPOS`): их результат лежит в общей памяти, выделенной под процесс окна, которое нашёл `FindWindow`,
/// поэтому такие сообщения должны попасть прямо в Проводник, а переслать их нельзя. Без хоста просто `f()`.
pub fn with_host_lowered<R>(f: impl FnOnce() -> R) -> R {
    let ours = OURS.get();
    let Some(explorer) = (ours != 0).then(explorer_tray).flatten() else { return f() };
    let ours = HWND(ours as *mut _);
    unsafe {
        let _ = SetWindowPos(ours, Some(explorer), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        let result = f();
        let _ = SetWindowPos(ours, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        result
    }
}

fn forward(msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(target) = explorer_tray() else { return LRESULT(0) };
    let mut result = 0usize;
    unsafe {
        SendMessageTimeoutW(target, msg, wp, lp, SMTO_ABORTIFHUNG, 2000, Some(&mut result));
    }
    LRESULT(result as isize)
}

fn notify_changed() {
    let (hwnd, msg) = NOTIFY.get();
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
            if cds.dwData == COPYDATA_TRAY && !cds.lpData.is_null() {
                let data = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
                return match parse_copydata(data) {
                    Some((cmd, n)) => {
                        // Будим панель, только если изменился сам набор или значок, а не подсказка.
                        let (before, after) = STATE.with(|s| {
                            let mut s = s.borrow_mut();
                            let before = s.signature();
                            s.apply(cmd, n);
                            (before, s.signature())
                        });
                        if before != after {
                            notify_changed();
                        }
                        LRESULT(1)
                    }
                    // Незнакомый формат: пусть разбирается Проводник, чем потерять значок.
                    None => forward(msg, wp, lp),
                };
            }
            forward(msg, wp, lp)
        }
        WM_COMMAND => forward(msg, wp, lp),
        m if (WM_USER..0x8000).contains(&m) => forward(msg, wp, lp),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

pub struct TrayHost {
    hwnd: HWND,
    claimed_existing: bool,
}

/// Просит приложения заново добавить значки трея. `HWND_BROADCAST` доходит только до окон верхнего уровня,
/// а многие программы (Qt, Electron, службы) слушают в скрытых окнах только для сообщений, поэтому
/// дополнительно обходим и их.
fn broadcast_taskbar_created() {
    unsafe {
        let msg = RegisterWindowMessageW(w!("TaskbarCreated"));
        let _ = PostMessageW(Some(HWND_BROADCAST), msg, WPARAM(0), LPARAM(0));
        let mut prev = None;
        while let Ok(h) = FindWindowExW(Some(HWND_MESSAGE), prev, PCWSTR::null(), PCWSTR::null()) {
            if h.0.is_null() {
                break;
            }
            let _ = PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0));
            prev = Some(h);
        }
    }
}

impl TrayHost {
    /// Создаёт окно `Shell_TrayWnd` над Проводником. `notify_hwnd` получает `notify_msg`, когда список значков
    /// меняется. `claim_existing`: разослать `TaskbarCreated`, чтобы уже запущенные приложения добавили значки заново
    /// (уже тут, а не в родной трей). Хост живёт в потоке с циклом сообщений.
    pub fn start(notify_hwnd: isize, notify_msg: u32, claim_existing: bool) -> Result<TrayHost> {
        if OURS.get() != 0 {
            bail!("tray host already running in this thread");
        }
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst.into(),
                lpszClassName: w!("Shell_TrayWnd"),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                w!("Shell_TrayWnd"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinst.into()),
                None,
            )?;
            OURS.set(hwnd.0 as isize);
            NOTIFY.set((notify_hwnd, notify_msg));
            STATE.with(|s| *s.borrow_mut() = TrayState::default());
            EXE_CACHE.with(|c| c.borrow_mut().clear());
            let host = TrayHost { hwnd, claimed_existing: claim_existing };
            host.raise();
            if claim_existing {
                broadcast_taskbar_created();
            }
            Ok(host)
        }
    }

    /// Система берёт начало анимации окон (появление, сворачивание, восстановление) от окна `Shell_TrayWnd`.
    /// Наше окно должно стоять там же, где настоящая панель задач, иначе окна «выезжают» из угла экрана.
    /// `true`, если положение пришлось поправить.
    pub fn sync_geometry(&self) -> bool {
        let Some(explorer) = explorer_tray() else { return false };
        unsafe {
            let (mut want, mut have) = (RECT::default(), RECT::default());
            if GetWindowRect(explorer, &mut want).is_err() || GetWindowRect(self.hwnd, &mut have).is_err() || want == have {
                return false;
            }
            let _ = SetWindowPos(
                self.hwnd,
                None,
                want.left,
                want.top,
                want.right - want.left,
                want.bottom - want.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            true
        }
    }

    /// Поднимает окно над Проводником: приложения ищут `Shell_TrayWnd` по Z-порядку.
    /// `true`, если пришлось поднять.
    pub fn raise(&self) -> bool {
        self.sync_geometry();
        unsafe {
            let first = FindWindowExW(None, None, w!("Shell_TrayWnd"), PCWSTR::null()).ok();
            if first.is_some_and(|h| h == self.hwnd) {
                return false;
            }
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            true
        }
    }

    /// Видимые значки; значки закрытых окон отбрасываются.
    pub fn icons(&self) -> Vec<TrayIcon> {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.prune(|h| unsafe { IsWindow(Some(HWND(h as *mut _))).as_bool() });
            s.visible().cloned().collect()
        })
    }

    /// Нажатие на значок: приложению уходят те же сообщения, что шлёт родной трей.
    pub fn send_click(&self, icon: &TrayIcon, button: Button) {
        if icon.callback_msg == 0 {
            return;
        }
        let target = HWND(icon.hwnd as *mut _);
        let (down, up, extra) = match button {
            Button::Left => (WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT),
            Button::Right => (WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU),
        };
        unsafe {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let mut pt = Default::default();
            let _ = GetCursorPos(&mut pt);
            // Версия 4: в `wParam` точка, в младшем слове `lParam` событие, в старшем номер значка.
            let v4 = icon.version >= NOTIFYICON_VERSION_4;
            let wp = if v4 { ((pt.y as usize & 0xFFFF) << 16) | (pt.x as usize & 0xFFFF) } else { icon.uid as usize };
            let lp = |ev: u32| if v4 { ((icon.uid << 16) | (ev & 0xFFFF)) as isize } else { ev as isize };
            let events: &[u32] = if v4 { &[down, up, extra] } else { &[down, up] };
            for &ev in events {
                let _ = PostMessageW(Some(target), icon.callback_msg, WPARAM(wp), LPARAM(lp(ev)));
            }
        }
    }

    /// Путь к exe приложения-владельца значка (для закрепления и подсказок).
    pub fn exe_of(&self, icon: &TrayIcon) -> Option<String> {
        // Путь не меняется, пока живо окно: запоминаем, чтобы не открывать процесс на каждый запрос.
        if let Some(hit) = EXE_CACHE.with(|c| c.borrow().get(&icon.hwnd).cloned()) {
            return hit;
        }
        let path = unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(HWND(icon.hwnd as *mut _), Some(&mut pid));
            exe_of_pid(pid)
        };
        EXE_CACHE.with(|c| c.borrow_mut().insert(icon.hwnd, path.clone()));
        path
    }
}

impl Drop for TrayHost {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        OURS.set(0);
        NOTIFY.set((0, 0));
        // Возвращаем значки Проводнику: приложения добавят их в родной трей заново.
        if self.claimed_existing {
            broadcast_taskbar_created();
        }
    }
}

impl TrayHost {
    /// Полоса пересобрана (новое окно): уведомления о смене значков теперь идут туда.
    pub fn set_notify(&self, hwnd: isize, msg: u32) {
        NOTIFY.set((hwnd, msg));
    }
}

/// Просит все приложения заново добавить значки в трей Проводника. Нужна, если macshell закрыт аварийно
/// и значки остались без хозяина (`macshell --quit` вызывает это).
pub fn restore_native_tray() {
    broadcast_taskbar_created();
}

/// Путь к exe процесса по его номеру. `None`, если процесс закрыт или недоступен (запущен от администратора).
pub fn exe_of_pid(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}
