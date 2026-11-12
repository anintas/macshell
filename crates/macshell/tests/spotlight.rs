//! Сквозные тесты Spotlight: нужен рабочий стол, другой экземпляр macshell не запущен (`macshell.exe --quit`).
//! Запуск: `cargo test -p macshell --test spotlight -- --ignored --nocapture --test-threads=1`.
//! Тесты нажимают настоящие клавиши через `SendInput`: не трогайте клавиатуру, пока они идут.

use std::sync::Arc;
use std::time::{Duration, Instant};

use macshell::host::{pump_messages, set_dpi_awareness};
use macshell::index::{Kind, Usage, build_index};
use macshell::spotlight::{Hit, Outcome, SpotlightWin, clipboard_text, set_clipboard_text};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
    VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LMENU, VK_LWIN, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};
use windows::core::{PCWSTR, w};

fn input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn press(vk: VIRTUAL_KEY) {
    send(&[input(vk.0, 0, KEYBD_EVENT_FLAGS(0))]);
    std::thread::sleep(Duration::from_millis(40));
    send(&[input(vk.0, 0, KEYEVENTF_KEYUP)]);
}

/// Win+Alt: оба нажаты, потом отпущены.
fn win_alt() {
    let pause = || std::thread::sleep(Duration::from_millis(40));
    send(&[input(VK_LWIN.0, 0, KEYBD_EVENT_FLAGS(0))]);
    pause();
    send(&[input(VK_LMENU.0, 0, KEYBD_EVENT_FLAGS(0))]);
    pause();
    send(&[input(VK_LMENU.0, 0, KEYEVENTF_KEYUP)]);
    pause();
    send(&[input(VK_LWIN.0, 0, KEYEVENTF_KEYUP)]);
}

fn chord(modifier: VIRTUAL_KEY, key: u16) {
    send(&[input(modifier.0, 0, KEYBD_EVENT_FLAGS(0)), input(key, 0, KEYBD_EVENT_FLAGS(0))]);
    std::thread::sleep(Duration::from_millis(40));
    send(&[input(key, 0, KEYEVENTF_KEYUP), input(modifier.0, 0, KEYEVENTF_KEYUP)]);
}

/// Набирает текст как Unicode-ввод: годится для любой раскладки.
fn type_text(text: &str) {
    for unit in text.encode_utf16() {
        send(&[input(0, unit, KEYEVENTF_UNICODE), input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)]);
        std::thread::sleep(Duration::from_millis(8));
    }
}

fn foreground_class() -> String {
    unsafe {
        let mut buf = [0u16; 64];
        let n = GetClassNameW(GetForegroundWindow(), &mut buf) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

fn layered_count() -> usize {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let (mut n, mut prev) = (0, None);
    unsafe {
        while let Ok(h) = FindWindowExW(None, prev, w!("macshell_layered"), PCWSTR::null()) {
            if h.0.is_null() {
                break;
            }
            n += 1;
            prev = Some(h);
        }
    }
    n
}

fn isolate() {
    let path = std::env::temp_dir().join("macshell-test-config.toml");
    std::fs::write(&path, "tray = \"off\"\n").unwrap();
    unsafe {
        std::env::set_var("MACSHELL_CONFIG", &path);
        std::env::set_var("MACSHELL_NO_CLAIM", "1");
    }
}

/// Крутит окно Spotlight `ms` миллисекунд как главный цикл панели; возвращает последнее не пустое решение окна.
fn settle(s: &mut SpotlightWin, ms: u64) -> Outcome {
    let end = Instant::now() + Duration::from_millis(ms);
    let mut last = Outcome::Stay;
    while Instant::now() < end {
        pump_messages(10);
        match s.handle_input() {
            Outcome::Stay => {
                s.present().unwrap();
                s.tick().unwrap();
            }
            other => last = other,
        }
    }
    last
}

fn first_title(s: &SpotlightWin) -> String {
    match s.hits().first() {
        Some(Hit::Calc(r)) => format!("={r}"),
        Some(Hit::Entry(e)) => e.title.clone(),
        Some(Hit::Convert(result, _)) => result.clone(),
        Some(_) | None => String::new(),
    }
}

#[test]
#[ignore]
fn win_tap_opens_spotlight_without_the_start_menu_and_escape_closes_it() {
    isolate();
    set_dpi_awareness().unwrap();
    let handle = std::thread::spawn(macshell::bar::run);
    std::thread::sleep(Duration::from_millis(2500));
    if handle.is_finished() {
        panic!("bar exited early: {:?}", handle.join().unwrap());
    }
    assert_eq!(layered_count(), 1, "only the bar window expected before the tap");

    // Одиночный Win не открывает Spotlight (это Пуск); сначала проверяем это.
    let tapped = Instant::now();
    win_alt();
    let mut appeared = None;
    while tapped.elapsed() < Duration::from_millis(2000) {
        if layered_count() == 2 {
            appeared = Some(tapped.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let appeared = appeared.expect("Spotlight window did not appear after a Win tap");
    println!("Win tap to window: {appeared:?}");
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(foreground_class(), "macshell_layered", "Spotlight must hold the focus; Start menu must stay closed");

    press(VK_ESCAPE);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(layered_count(), 1, "Escape must close Spotlight");
    assert_ne!(foreground_class(), "macshell_layered");

    // Второе нажатие открывает снова, третье закрывает (переключатель).
    win_alt();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(layered_count(), 2);
    win_alt();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(layered_count(), 1, "second tap toggles Spotlight off");

    assert!(macshell::bar::request_quit());
    handle.join().unwrap().unwrap();
}

#[test]
#[ignore]
fn spotlight_window_takes_typing_selection_paste_and_finds_things() {
    set_dpi_awareness().unwrap();
    let t = Instant::now();
    let index = Arc::new(build_index());
    println!("index: {} entries in {:?}", index.len(), t.elapsed());
    assert!(index.iter().any(|e| e.kind == Kind::App));

    let opened = Instant::now();
    let mut s = SpotlightWin::open(index.clone(), &Usage::default(), None, Arc::new(Vec::new())).unwrap();
    let open_time = opened.elapsed();
    println!("open: {open_time:?}");
    settle(&mut s, 300);
    assert!(s.has_focus(), "Spotlight must take the keyboard focus");

    type_text("calc");
    settle(&mut s, 250);
    assert_eq!(s.query(), "calc");
    println!("calc -> {}", first_title(&s));
    assert!(
        s.hits().iter().any(|h| matches!(h, Hit::Entry(e) if e.target.to_lowercase().contains("calc") || e.title.contains("Калькулятор"))),
        "calculator app must be among the results: {:?}",
        s.hits().iter().map(|h| first_of(h)).collect::<Vec<_>>()
    );

    for _ in 0..4 {
        press(VK_BACK);
    }
    settle(&mut s, 200);
    assert_eq!(s.query(), "");
    assert!(s.hits().is_empty());

    type_text("звук");
    settle(&mut s, 250);
    assert_eq!(first_title(&s), "Звук");

    chord(VK_CONTROL, VK_BACK.0);
    settle(&mut s, 150);
    assert_eq!(s.query(), "", "Ctrl+Backspace removes the word");

    type_text("ыщгтв"); // «sound» в русской раскладке
    settle(&mut s, 250);
    assert_eq!(first_title(&s), "Звук", "wrong-layout query must still find Звук");

    for _ in 0..5 {
        press(VK_BACK);
    }
    type_text("2+2*3");
    settle(&mut s, 250);
    assert_eq!(first_title(&s), "=8");

    for _ in 0..5 {
        press(VK_BACK);
    }
    type_text("настр");
    settle(&mut s, 250);
    let count = s.hits().len();
    assert!(count >= 2, "need several results for selection, got {count}");
    press(VK_DOWN);
    press(VK_TAB);
    settle(&mut s, 150); // не падает; выбор проверен модульно

    // Вставка из буфера обмена: сохраняем и возвращаем то, что лежало.
    let saved = clipboard_text();
    assert!(set_clipboard_text("хром"));
    for _ in 0..5 {
        press(VK_BACK);
    }
    chord(VK_CONTROL, 0x56);
    settle(&mut s, 250);
    if let Some(old) = saved {
        set_clipboard_text(&old);
    }
    assert_eq!(s.query(), "хром", "Ctrl+V must paste the clipboard text");

    press(VK_ESCAPE);
    let outcome = settle(&mut s, 200);
    assert!(matches!(outcome, Outcome::Close), "Escape must ask to close, got {outcome:?}");
    assert!(open_time < Duration::from_millis(400), "open took {open_time:?}");
}

fn first_of(h: &Hit) -> String {
    match h {
        Hit::Calc(r) => format!("={r}"),
        Hit::Entry(e) => e.title.clone(),
        Hit::Convert(result, _) => result.clone(),
        Hit::File(_) | Hit::Action(_) | Hit::Clip(..) | Hit::Quick(_) => String::new(),
    }
}
