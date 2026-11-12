//! Interoperability check: Rust publishes and two C++ readers open the same image.
fn main() {
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};
    use windows::core::w;
    unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("caption shared test"), WS_POPUP,
            0, 0, 100, 100, None, None, None, None).unwrap();
        let map = macshell::caption_shared::CaptionMapping::new(std::process::id(), hwnd.0 as isize, 8).unwrap();
        assert!(map.publish(2, &[0xff112233; 16]));
        for exe in ["target/caption-snapshot-test64.exe", "target/caption-snapshot-test32.exe"] {
            let status = std::process::Command::new(exe).arg((hwnd.0 as usize).to_string()).status().unwrap();
            assert!(status.success(), "reader failed: {exe}");
        }
        DestroyWindow(hwnd).unwrap();
    }
}
