//! Временная проба системного оформления одного окна Проводника: без внедрения DLL.
//! `cargo run -p macshell --example explorer_preview -- <HWND> [--preview]`.
//! Меняет только атрибуты, чьи исходные значения удалось прочитать; Enter возвращает их назад.

use std::io;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWINDOWATTRIBUTE,
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWA_SYSTEMBACKDROP_TYPE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, IsWindow};

struct Restore {
    hwnd: HWND,
    attributes: Vec<(DWMWINDOWATTRIBUTE, u32)>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        for (attribute, value) in self.attributes.iter().rev() {
            let result = unsafe { DwmSetWindowAttribute(self.hwnd, *attribute, value as *const _ as *const _, 4) };
            println!("restore {}: {}", attribute.0, if result.is_ok() { "ok" } else { "failed" });
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let hwnd = HWND(args.next().ok_or_else(|| anyhow::anyhow!("expected Explorer HWND"))?.parse::<isize>()? as *mut _);
    let preview = args.any(|arg| arg == "--preview");
    let mut name = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
    anyhow::ensure!(unsafe { IsWindow(Some(hwnd)) }.as_bool() && String::from_utf16_lossy(&name[..len]) == "CabinetWClass", "not a File Explorer window");
    let attributes = [
        ("dark", DWMWA_USE_IMMERSIVE_DARK_MODE, 1),
        ("corners", DWMWA_WINDOW_CORNER_PREFERENCE, 2),
        ("border", DWMWA_BORDER_COLOR, 0x00625855),
        ("caption", DWMWA_CAPTION_COLOR, 0x001e1c1c),
        ("text", DWMWA_TEXT_COLOR, 0x00eeeeee),
        ("backdrop", DWMWA_SYSTEMBACKDROP_TYPE, 2),
    ];
    let mut restore = Restore { hwnd, attributes: Vec::new() };
    for (name, attribute, desired) in attributes {
        let mut original = 0u32;
        let read = unsafe { DwmGetWindowAttribute(hwnd, attribute, &mut original as *mut _ as *mut _, 4) };
        match read {
            Ok(()) => {
                println!("{name}: original={original:#x}");
                if preview {
                    let result = unsafe { DwmSetWindowAttribute(hwnd, attribute, &desired as *const _ as *const _, 4) };
                    println!("{name}: {}", if result.is_ok() { "applied" } else { "unsupported" });
                    if result.is_ok() { restore.attributes.push((attribute, original)); }
                }
            }
            Err(_) => println!("{name}: unreadable, skipped"),
        }
    }
    if preview && !restore.attributes.is_empty() {
        println!("Preview active. Press Enter to restore.");
        let _ = io::stdin().read_line(&mut String::new());
    }
    Ok(())
}
