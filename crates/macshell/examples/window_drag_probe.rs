//! A temporary isolated hook for manual testing against a separate test window.
fn main() {
    macshell::host::set_dpi_awareness().expect("DPI");
    let _drag = macshell::window_drag::WindowDrag::start().expect("drag hook");
    std::thread::sleep(std::time::Duration::from_secs(180));
}
