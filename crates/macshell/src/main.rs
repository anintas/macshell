// В релизе без консоли: оболочка не должна открывать окно терминала при запуске.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};

use anyhow::Result;
use capture::{RectI, capture_region};
use glass::gpu::{GpuDriver, GpuGlass};
use glass::{Bitmap, GlassParams, cpu};
use macshell::host::{LayeredWindow, pump_messages, set_dpi_awareness};
use macshell::snapshot::write_bmp;
use macshell::slint_view::ContentView;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

enum GlassMode {
    Cpu,
    Gpu(GpuGlass),
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let animate = args.iter().any(|a| a == "--animate");
    let snapshot = args.iter().find_map(|a| a.strip_prefix("--snapshot=")).map(String::from);
    match args.first().map(String::as_str) {
        Some("proto-cpu") => run_proto(GlassMode::Cpu, animate, snapshot),
        Some("proto-gpu") => run_proto(GlassMode::Gpu(GpuGlass::new(GpuDriver::Hardware)?), animate, snapshot),
        Some("--quit") => {
            if !macshell::bar::request_quit() {
                // Полосы нет (закрыта аварийно): вернуть значки родному трею.
                macshell::tray::host::restore_native_tray();
                macshell::taskbar::restore_from_file();
                eprintln!("macshell is not running");
            }
            Ok(())
        }
        // Родная панель задач осталась спрятанной после аварии: вернуть её.
        Some("--restore-taskbar") => {
            if !macshell::taskbar::restore_from_file() {
                eprintln!("nothing to restore");
            }
            Ok(())
        }
        None => macshell::bar::run(),
        Some("--restart") => {
            // Старая копия ещё закрывается: пробуем занять замок до 3 секунд.
            for _ in 0..15 {
                match macshell::bar::run() {
                    Err(e) if e.downcast_ref::<macshell::bar::AlreadyRunning>().is_some() => {
                        std::thread::sleep(Duration::from_millis(200))
                    }
                    other => return other,
                }
            }
            Err(macshell::bar::AlreadyRunning.into())
        }
        Some(s) if s.starts_with("--snapshot=") => macshell::bar::snapshot(&s["--snapshot=".len()..], false),
        Some(s) if s.starts_with("--snapshot-solid=") => macshell::bar::snapshot(&s["--snapshot-solid=".len()..], true),
        Some(s) if s.starts_with("--snapshot-tray=") => macshell::bar::snapshot_tray(&s["--snapshot-tray=".len()..], false),
        Some(s) if s.starts_with("--snapshot-tray-mono=") => macshell::bar::snapshot_tray(&s["--snapshot-tray-mono=".len()..], true),
        _ => {
            eprintln!("usage: macshell [--quit | --snapshot=out.bmp]");
            eprintln!("       macshell proto-cpu|proto-gpu [--animate] [--snapshot=out.bmp]   (exit: Ctrl+Shift+Q)");
            Ok(())
        }
    }
}

/// Размер панели в логических пикселях, как `preferred-*` в ui/proto.slint.
const PANEL_W: f32 = 680.0;
const PANEL_H: f32 = 353.0;

/// Тёмное дымчатое стекло под белый текст. Радиус 30 = радиус строки 18 + отступ 12 (концентрично).
fn spotlight_glass() -> GlassParams {
    GlassParams {
        radius: 30.0,
        bevel: 24.0,
        refraction: 16.0,
        blur_radius: 12,
        saturation: 1.5,
        tint: [0.05, 0.06, 0.09, 0.36],
        dispersion: 2.0,
        rim: 0.6,
        ..GlassParams::default()
    }
}


fn quit_requested() -> bool {
    unsafe {
        let held = |vk: i32| GetAsyncKeyState(vk) as u16 & 0x8000 != 0;
        let q_pressed = GetAsyncKeyState(0x51) as u16 & 0x8001 != 0;
        held(VK_CONTROL.0 as i32) && held(VK_SHIFT.0 as i32) && q_pressed
    }
}

fn hash_pixels(px: &[u32]) -> u64 {
    let mut h = DefaultHasher::new();
    px.hash(&mut h);
    h.finish()
}

fn compose(backdrop: &Bitmap, content: &[u32], params: &GlassParams, mode: &GlassMode) -> Result<Bitmap> {
    let mut out = match mode {
        GlassMode::Cpu => cpu::render(backdrop, params),
        GlassMode::Gpu(gpu) => gpu.render(backdrop, params)?,
    };
    cpu::composite_over(&mut out, content);
    Ok(out)
}


fn run_proto(mode: GlassMode, animate: bool, snapshot: Option<String>) -> Result<()> {
    set_dpi_awareness()?;
    let scale = unsafe { GetDpiForSystem() } as f32 / 96.0;
    let (w, h) = ((PANEL_W * scale).round() as i32, (PANEL_H * scale).round() as i32);
    let x = (unsafe { GetSystemMetrics(SM_CXSCREEN) } - w) / 2;
    let base_y = (unsafe { GetSystemMetrics(SM_CYSCREEN) } as f32 * 0.16).round() as i32;

    let win = LayeredWindow::new(RectI { x, y: base_y, w, h })?;
    let view = ContentView::new(w as u32, h as u32);
    view.set_scale_factor(scale);
    let content = view.render();
    let params = spotlight_glass().scaled(scale);

    let start = Instant::now();
    let mut last_hash = None;
    let (mut frames, mut spent) = (0u32, Duration::ZERO);
    loop {
        if quit_requested() {
            return Ok(());
        }
        let t0 = Instant::now();
        if animate {
            let phase = start.elapsed().as_secs_f32() * std::f32::consts::TAU;
            win.set_position(x, base_y + (12.0 * scale * phase.sin()).round() as i32)?;
        }
        let backdrop = capture_region(win.rect())?;
        let hash = hash_pixels(&backdrop.pixels);
        if animate || last_hash != Some(hash) {
            let out = compose(&backdrop, &content, &params, &mode)?;
            win.present(&out)?;
            if let Some(path) = &snapshot {
                let mut shot = backdrop.clone();
                cpu::composite_over(&mut shot, &out.pixels);
                return write_bmp(path, &shot);
            }
            last_hash = Some(hash);
            frames += 1;
            spent += t0.elapsed();
            if animate && frames % 120 == 0 {
                eprintln!("avg frame: {:.2} ms", spent.as_secs_f64() * 1000.0 / frames as f64);
            }
        }
        if !pump_messages(if animate { 16 } else { 250 }) {
            return Ok(());
        }
    }
}
