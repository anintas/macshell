//! Applies the experimental caption only to the dedicated MacshellDragProbe test window.
fn main() {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::w;
    macshell::host::set_dpi_awareness().expect("DPI");
    let target = unsafe { FindWindowW(w!("MacshellDragProbe"), None) }.expect("start drag-probe.exe first");
    let _probe = macshell::window_chrome::CaptionProbe::start_for(target.0 as isize).expect("caption probe");
    std::thread::sleep(std::time::Duration::from_secs(180));
}
