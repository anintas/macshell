//! Разведка: подходит ли штатный бэкенд Slint (winit) для Spotlight — прозрачность и ввод по-русски.

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let ui = Spike::new()?;
    ui.on_close_requested(|| {
        let _ = slint::quit_event_loop();
    });
    ui.run()
}
