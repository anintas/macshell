//! Сочетание Win+Alt (без третьей клавиши) открывает Spotlight. Одиночное нажатие Win открывает Dock и сетку
//! приложений вместо Пуска (`win_start = "keep"` в конфиге возвращает Пуск); Win с любой другой клавишей работает как
//! обычно.
//! Хук `WH_KEYBOARD_LL` живёт в своём потоке, обработчик короткий: автомат нажатий, одна пустая клавиша и одно
//! сообщение окну панели.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::thread::JoinHandle;

use anyhow::{Result, bail};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, MSG, PostMessageW, PostThreadMessageW,
    KillTimer, SetTimer, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
    WM_TIMER,
};

/// Дольше этого от первого нажатия до первого отпускания сочетание считается удержанием: ничего не открывается.
pub const MAX_CHORD_MS: u64 = 1000;
/// Одиночный Win, который держали дольше, всё равно открывает сетку: Пуск тоже открывается при любом удержании.
pub const MAX_TAP_MS: u64 = 10_000;
/// Пометка собственных внедрённых клавиш: хук их не разбирает.
const MAGIC: usize = 0x6D61_6373;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// Как часто хук ставится заново, миллисекунды.
const REINSTALL_MS: u32 = 20_000;
const VK_MENU: u32 = 0x12;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
/// Не назначенная клавиша: между нажатием и отпусканием Win или Alt она отменяет их собственное действие
/// (меню Пуск у Win, переход в меню окна у Alt).
const VK_NOOP: u16 = 0xE8;
const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_LEFT: u32 = 0x25;
const VK_RIGHT: u32 = 0x27;
const VK_Q: u32 = 0x51;
const VK_H: u32 = 0x48;
/// Клавиша с обратной кавычкой (Ё в русской раскладке): Alt+` переключает окна одного приложения.
const VK_OEM_3: u32 = 0xC0;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
/// У события клавиатуры в `flags`: Alt нажат.
const LLKHF_ALTDOWN: u32 = 0x20;

/// Что хук сообщает окну панели про переключатель приложений (в `wparam` уходит `10 + номер`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    Next = 1,
    Prev = 2,
    /// Alt отпустили: переключиться на выбранное.
    Commit = 3,
    Cancel = 4,
    /// Q: закрыть выбранное приложение.
    Quit = 5,
    /// H: спрятать выбранное приложение.
    Hide = 6,
    /// Alt+`: следующее окно того же приложения.
    SameApp = 7,
}

/// Решение для клавиши, когда переключатель включён: что отправить, глотать ли клавишу и идёт ли после неё
/// переключение. Чистая логика без системных вызовов.
pub fn switch_decision(vk: u32, down: bool, alt: bool, win: bool, switching: bool, shift: bool, ctrl: bool) -> (Option<Switch>, bool, bool) {
    // Переключатель держится на Alt или на Win: Alt+Tab и Win+Tab открывают одно и то же, отпускание клавиши переключает.
    let is_modifier = matches!(vk, VK_MENU | VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN);
    match (vk, down) {
        (VK_TAB, true) if (alt || win) && !ctrl => (Some(if shift { Switch::Prev } else { Switch::Next }), true, true),
        (VK_TAB, false) if switching => (None, true, true),
        (VK_OEM_3, true) if alt && !switching && !ctrl => (Some(Switch::SameApp), true, false),
        (VK_ESCAPE, true) if switching => (Some(Switch::Cancel), true, false),
        (VK_LEFT, true) if switching => (Some(Switch::Prev), true, true),
        (VK_RIGHT, true) if switching => (Some(Switch::Next), true, true),
        (VK_Q, true) if switching => (Some(Switch::Quit), true, true),
        (VK_H, true) if switching => (Some(Switch::Hide), true, true),
        (VK_LEFT | VK_RIGHT | VK_Q | VK_H, false) if switching => (None, true, true),
        (_, false) if switching && is_modifier => (Some(Switch::Commit), false, false),
        _ => (None, false, switching),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Win,
    Alt,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Событие идёт в систему как есть.
    Pass,
    /// Win и Alt нажаты вместе без третьей клавиши и одну из них отпустили: гасим Пуск и меню, открываем Spotlight.
    Chord,
    /// Win нажали и отпустили сам по себе: гасим Пуск, открываем Dock с сеткой приложений.
    Tap,
}

/// Автомат сочетания Win+Alt в любом порядке нажатия. Чистая логика без системных вызовов.
#[derive(Debug, Default)]
pub struct TapDetector {
    win: bool,
    alt: bool,
    /// Время первого нажатия в текущей серии (пока нажата хоть одна из двух клавиш).
    started: Option<u64>,
    both_held: bool,
    other_pressed: bool,
    fired: bool,
}

impl TapDetector {
    pub fn new() -> TapDetector {
        TapDetector::default()
    }

    /// Следующее событие клавиатуры: `at_ms` время в миллисекундах, `ours` пометка собственной клавиши.
    pub fn feed(&mut self, key: Key, down: bool, at_ms: u64, ours: bool) -> Action {
        if ours {
            return Action::Pass;
        }
        match (key, down) {
            (Key::Win | Key::Alt, true) => {
                // Автоповтор не сдвигает время первого нажатия.
                if !self.win && !self.alt {
                    *self = TapDetector { started: Some(at_ms), ..TapDetector::default() };
                }
                if key == Key::Win {
                    self.win = true;
                } else {
                    self.alt = true;
                }
                self.both_held |= self.win && self.alt;
                Action::Pass
            }
            (Key::Win | Key::Alt, false) => {
                let was_held = if key == Key::Win { self.win } else { self.alt };
                if key == Key::Win {
                    self.win = false;
                } else {
                    self.alt = false;
                }
                let on_time = matches!(self.started, Some(t) if at_ms.saturating_sub(t) <= MAX_CHORD_MS);
                let fire = was_held && self.both_held && !self.other_pressed && !self.fired && on_time;
                let tap_on_time = matches!(self.started, Some(t) if at_ms.saturating_sub(t) <= MAX_TAP_MS);
                let tap = key == Key::Win && was_held && !self.both_held && !self.other_pressed && !self.fired && tap_on_time;
                self.fired |= fire;
                if !self.win && !self.alt {
                    *self = TapDetector::default();
                }
                if fire {
                    Action::Chord
                } else if tap {
                    Action::Tap
                } else {
                    Action::Pass
                }
            }
            (Key::Other, true) => {
                if self.win || self.alt {
                    self.other_pressed = true;
                }
                Action::Pass
            }
            (Key::Other, false) => Action::Pass,
        }
    }

    /// Забыть начатое нажатие (перехват выключили посреди нажатия).
    pub fn reset(&mut self) {
        *self = TapDetector::default();
    }
}

static ENABLED: AtomicBool = AtomicBool::new(true);
/// Одиночный Win открывает Dock и сетку приложений (иначе остаётся Пуском).
static TAP: AtomicBool = AtomicBool::new(true);
/// Alt+Tab открывает свой переключатель приложений.
static SWITCHER: AtomicBool = AtomicBool::new(true);
/// Переключатель сейчас открыт: Alt держат после Tab.
static SWITCHING: AtomicBool = AtomicBool::new(false);
/// Win сейчас нажата (по событиям хука): с ней Tab открывает переключатель.
static WIN_DOWN: AtomicBool = AtomicBool::new(false);
static TARGET: AtomicIsize = AtomicIsize::new(0);
static MESSAGE: AtomicU32 = AtomicU32::new(0);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static DETECTOR: RefCell<TapDetector> = RefCell::new(TapDetector::new());
}

/// Включает и выключает перехват (в полноэкранных играх сочетания работают как обычно).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

/// Включает и выключает замену Пуска на сетку приложений.
pub fn set_tap(on: bool) {
    TAP.store(on, Ordering::SeqCst);
}

/// Включает и выключает свой переключатель приложений (иначе Alt+Tab остаётся системным).
pub fn set_switcher(on: bool) {
    SWITCHER.store(on, Ordering::SeqCst);
}

/// Окно, которому хук шлёт сообщение; панель пересобирается, окно меняется.
pub fn set_target(hwnd: isize) {
    TARGET.store(hwnd, Ordering::SeqCst);
}

/// Нажимает сочетание клавиш и отпускает его (виртуальный код, стрелка ли: у стрелок флаг расширенной клавиши). Клавиши
/// помечены `MAGIC`: хук их не разбирает. Так переключаются рабочие столы Windows (Ctrl+Win+стрелка).
pub fn send_chord(keys: &[(u16, bool)]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY;
    let input = |vk: u16, ext: bool, up: bool| {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        if ext {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } } }
    };
    let mut inputs: Vec<INPUT> = keys.iter().map(|(vk, ext)| input(*vk, *ext, false)).collect();
    inputs.extend(keys.iter().rev().map(|(vk, ext)| input(*vk, *ext, true)));
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Нажимает и отпускает не назначенную клавишу. Хук её не разбирает (метка `MAGIC`). После такого ввода от нашего
/// процесса система разрешает ему забрать передний план, а между нажатием и отпусканием Win или Alt она отменяет
/// их собственное действие.
pub fn send_noop_key() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(VK_NOOP), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: MAGIC },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
        let key = match info.vkCode {
            VK_LWIN | VK_RWIN => Key::Win,
            VK_MENU | VK_LMENU | VK_RMENU => Key::Alt,
            _ => Key::Other,
        };
        let ours = info.dwExtraInfo == MAGIC;
        if !ours && key == Key::Win {
            WIN_DOWN.store(down, Ordering::SeqCst);
        }
        if !ours && SWITCHER.load(Ordering::Relaxed) && ENABLED.load(Ordering::Relaxed) {
            let alt = info.flags.0 & LLKHF_ALTDOWN != 0;
            let (shift, ctrl) = unsafe { (GetAsyncKeyState(VK_SHIFT) as u16 & 0x8000 != 0, GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000 != 0) };
            let switching = SWITCHING.load(Ordering::SeqCst);
            let win = WIN_DOWN.load(Ordering::SeqCst);
            let (send, swallow, now_switching) = switch_decision(info.vkCode, down, alt, win, switching, shift, ctrl);
            SWITCHING.store(now_switching, Ordering::SeqCst);
            if let Some(what) = send {
                if what == Switch::Commit {
                    send_noop_key(); // отпускание Alt не должно открыть меню окна
                }
                let target = TARGET.load(Ordering::SeqCst);
                if target != 0 {
                    unsafe {
                        let _ = PostMessageW(Some(HWND(target as *mut _)), MESSAGE.load(Ordering::SeqCst), WPARAM(10 + what as usize), LPARAM(0));
                    }
                }
            }
            if swallow {
                // Съеденная клавиша для автомата Win и Alt всё равно «третья»: иначе Win+Tab засчитался бы одиночным Win.
                if down {
                    DETECTOR.with(|d| d.borrow_mut().feed(Key::Other, true, info.time as u64, false));
                }
                return LRESULT(1);
            }
        }
        let action = DETECTOR.with(|d| {
            let mut d = d.borrow_mut();
            if !ENABLED.load(Ordering::Relaxed) {
                d.reset();
                return Action::Pass;
            }
            d.feed(key, down, info.time as u64, ours)
        });
        let action = if action == Action::Tap && !TAP.load(Ordering::Relaxed) { Action::Pass } else { action };
        if action != Action::Pass {
            send_noop_key();
            let target = TARGET.load(Ordering::SeqCst);
            if target != 0 {
                // `wparam`: 0 Win+Alt (Spotlight), 1 одиночный Win (сетка приложений).
                let kind = if action == Action::Tap { 1 } else { 0 };
                unsafe {
                    let _ = PostMessageW(Some(HWND(target as *mut _)), MESSAGE.load(Ordering::SeqCst), WPARAM(kind), LPARAM(0));
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Работающий хук. Пока значение живо, перехват включён; при уничтожении хук снимается.
pub struct WinKey {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

impl WinKey {
    /// Ставит хук в отдельном потоке. `target` получает `msg` при каждом сочетании Win+Alt.
    pub fn start(target: isize, msg: u32) -> Result<WinKey> {
        set_target(target);
        MESSAGE.store(msg, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel::<Result<u32, String>>();
        let thread = std::thread::Builder::new().name("winkey".into()).spawn(move || unsafe {
            let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
            let mut hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(hinst), 0) {
                Ok(h) => Some(h),
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };
            // Система молча снимает хук, который не успел ответить за отведённое время (сильная нагрузка, медленный
            // хук другой программы). Поэтому хук ставится заново раз в REINSTALL_MS: сбой длится не дольше этого.
            let timer = SetTimer(None, 0, REINSTALL_MS, None);
            let id = GetCurrentThreadId();
            THREAD_ID.store(id, Ordering::SeqCst);
            let _ = tx.send(Ok(id));
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_TIMER && msg.wParam.0 == timer {
                    if let Some(old) = hook.take() {
                        let _ = UnhookWindowsHookEx(old);
                    }
                    hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(hinst), 0).ok();
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = KillTimer(None, timer);
            if let Some(h) = hook {
                let _ = UnhookWindowsHookEx(h);
            }
        })?;
        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(WinKey { thread: Some(thread), thread_id }),
            Ok(Err(e)) => bail!("cannot install keyboard hook: {e}"),
            Err(_) => bail!("hook thread exited before installing the hook"),
        }
    }
}

impl Drop for WinKey {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Key::{Alt, Other, Win};

    fn run(events: &[(Key, bool, u64)]) -> Vec<Action> {
        let mut d = TapDetector::new();
        events.iter().map(|&(k, down, t)| d.feed(k, down, t, false)).collect()
    }

    fn fired(events: &[(Key, bool, u64)]) -> usize {
        run(events).iter().filter(|a| **a == Action::Chord).count()
    }

    fn decide(vk: u32, down: bool, alt: bool, switching: bool) -> (Option<Switch>, bool, bool) {
        switch_decision(vk, down, alt, false, switching, false, false)
    }

    #[test]
    fn win_tab_opens_the_same_switcher_and_releasing_win_commits() {
        assert_eq!(switch_decision(VK_TAB, true, false, true, false, false, false), (Some(Switch::Next), true, true));
        assert_eq!(switch_decision(VK_LWIN, false, false, false, true, false, false), (Some(Switch::Commit), false, false));
        assert_eq!(switch_decision(VK_RWIN, false, false, false, true, false, false), (Some(Switch::Commit), false, false));
        assert_eq!(switch_decision(VK_TAB, true, false, false, false, false, false), (None, false, false), "Tab alone is untouched");
    }

    #[test]
    fn a_swallowed_win_tab_does_not_count_as_a_lone_win_tap() {
        // Хук скармливает автомату съеденный Tab как «другую клавишу»: Win отпускают без одиночного нажатия.
        assert_eq!(taps(&[(Win, true, 0), (Other, true, 30), (Win, false, 90)]), 0);
    }

    #[test]
    fn alt_tab_starts_the_switcher_and_swallows_tab_in_both_directions() {
        assert_eq!(decide(VK_TAB, true, true, false), (Some(Switch::Next), true, true));
        assert_eq!(switch_decision(VK_TAB, true, true, false, true, true, false), (Some(Switch::Prev), true, true), "Shift+Tab goes back");
        assert_eq!(decide(VK_TAB, false, true, true), (None, true, true), "the release is swallowed too");
        assert_eq!(decide(VK_TAB, true, false, false), (None, false, false), "plain Tab is untouched");
        assert_eq!(switch_decision(VK_TAB, true, true, false, false, false, true), (None, false, false), "Ctrl+Alt+Tab stays with Windows");
    }

    #[test]
    fn releasing_alt_commits_once_and_ends_the_session() {
        assert_eq!(decide(VK_LMENU, false, false, true), (Some(Switch::Commit), false, false));
        assert_eq!(decide(VK_MENU, false, false, true), (Some(Switch::Commit), false, false));
        assert_eq!(decide(VK_LMENU, false, false, false), (None, false, false), "an Alt release with no session is ignored");
    }

    #[test]
    fn switcher_keys_work_only_while_it_is_open() {
        for (vk, what) in [(VK_ESCAPE, Switch::Cancel), (VK_LEFT, Switch::Prev), (VK_RIGHT, Switch::Next), (VK_Q, Switch::Quit), (VK_H, Switch::Hide)] {
            let (send, swallow, _) = decide(vk, true, true, true);
            assert_eq!((send, swallow), (Some(what), true), "{vk:#x}");
            assert_eq!(decide(vk, true, true, false), (None, false, false), "{vk:#x} passes through when closed");
        }
        assert_eq!(decide(VK_ESCAPE, true, true, true).2, false, "Esc ends the session");
        assert_eq!(decide(VK_Q, true, true, true).2, true);
    }

    #[test]
    fn alt_backtick_cycles_windows_of_one_app_without_a_session() {
        assert_eq!(decide(VK_OEM_3, true, true, false), (Some(Switch::SameApp), true, false));
        assert_eq!(decide(VK_OEM_3, true, false, false), (None, false, false));
    }

    #[test]
    fn win_then_alt_fires_once_on_the_first_release() {
        let out = run(&[(Win, true, 0), (Alt, true, 50), (Alt, false, 120), (Win, false, 150)]);
        assert_eq!(out, vec![Action::Pass, Action::Pass, Action::Chord, Action::Pass]);
    }

    #[test]
    fn alt_then_win_works_too_and_either_release_order_fires_once() {
        assert_eq!(fired(&[(Alt, true, 0), (Win, true, 40), (Win, false, 100), (Alt, false, 130)]), 1);
        assert_eq!(fired(&[(Win, true, 0), (Alt, true, 40), (Win, false, 100), (Alt, false, 130)]), 1);
    }

    #[test]
    fn win_alone_and_alt_alone_do_not_open_spotlight() {
        assert_eq!(fired(&[(Win, true, 0), (Win, false, 80)]), 0);
        assert_eq!(fired(&[(Alt, true, 0), (Alt, false, 80)]), 0);
    }

    fn taps(events: &[(Key, bool, u64)]) -> usize {
        run(events).iter().filter(|a| **a == Action::Tap).count()
    }

    #[test]
    fn a_lone_win_press_is_a_tap_and_any_other_key_or_alt_cancels_it() {
        assert_eq!(taps(&[(Win, true, 0), (Win, false, 80)]), 1);
        assert_eq!(taps(&[(Win, true, 0), (Win, true, 400), (Win, false, 500)]), 1, "autorepeat still one tap");
        assert_eq!(taps(&[(Win, true, 0), (Other, true, 30), (Other, false, 60), (Win, false, 90)]), 0, "Win+E is a shortcut");
        assert_eq!(taps(&[(Win, true, 0), (Alt, true, 30), (Alt, false, 60), (Win, false, 90)]), 0, "Win+Alt is the chord");
        assert_eq!(taps(&[(Win, true, 0), (Win, false, MAX_CHORD_MS + 1)]), 1, "a slow tap still replaces Start");
        assert_eq!(taps(&[(Win, true, 0), (Win, false, MAX_TAP_MS + 1)]), 0, "a very long hold is left to Windows");
        assert_eq!(taps(&[(Alt, true, 0), (Alt, false, 80)]), 0);
        assert_eq!(taps(&[(Win, false, 80)]), 0, "a release without a press");
    }

    #[test]
    fn two_taps_in_a_row_both_count() {
        assert_eq!(taps(&[(Win, true, 0), (Win, false, 60), (Win, true, 300), (Win, false, 360)]), 2);
    }

    #[test]
    fn a_third_key_cancels_the_chord() {
        // Win+Alt+R (Game Bar) и подобные не должны открывать Spotlight.
        assert_eq!(fired(&[(Win, true, 0), (Alt, true, 30), (Other, true, 60), (Other, false, 90), (Alt, false, 120), (Win, false, 150)]), 0);
        assert_eq!(fired(&[(Win, true, 0), (Other, true, 30), (Alt, true, 60), (Alt, false, 90), (Win, false, 120)]), 0);
    }

    #[test]
    fn autorepeat_does_not_restart_the_chord() {
        let events = [(Win, true, 0), (Alt, true, 10), (Win, true, 400), (Alt, true, 430), (Win, true, 800), (Alt, false, 900), (Win, false, 950)];
        assert_eq!(fired(&events), 1);
    }

    #[test]
    fn long_hold_is_not_a_chord() {
        assert_eq!(fired(&[(Win, true, 0), (Alt, true, 10), (Alt, false, MAX_CHORD_MS + 1), (Win, false, MAX_CHORD_MS + 20)]), 0);
        assert_eq!(fired(&[(Win, true, 0), (Alt, true, 10), (Alt, false, MAX_CHORD_MS), (Win, false, MAX_CHORD_MS + 20)]), 1);
    }

    #[test]
    fn own_injected_keys_are_ignored() {
        let mut d = TapDetector::new();
        d.feed(Win, true, 0, false);
        d.feed(Alt, true, 10, false);
        // Пустая клавиша, внедрённая нами, не считается третьей.
        assert_eq!(d.feed(Other, true, 20, true), Action::Pass);
        assert_eq!(d.feed(Alt, false, 30, false), Action::Chord);
    }

    #[test]
    fn two_chords_in_a_row_both_count() {
        let events = [
            (Win, true, 0),
            (Alt, true, 10),
            (Alt, false, 60),
            (Win, false, 70),
            (Win, true, 400),
            (Alt, true, 410),
            (Alt, false, 460),
            (Win, false, 470),
        ];
        assert_eq!(fired(&events), 2);
    }

    #[test]
    fn releases_without_presses_are_ignored() {
        // Клавиши были нажаты до запуска хука: видим только отпускание.
        assert_eq!(fired(&[(Alt, false, 100), (Win, false, 110), (Other, true, 120), (Other, false, 140)]), 0);
    }

    #[test]
    fn other_key_held_before_does_not_poison_a_later_chord() {
        assert_eq!(fired(&[(Other, true, 0), (Win, true, 50), (Alt, true, 60), (Alt, false, 100), (Win, false, 120), (Other, false, 140)]), 1);
    }
}
