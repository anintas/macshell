//! Сравнение значка 16 x 16 пикселей: растяжка `DrawIconEx` (было) и масштаб Ланцоша (стало). Нужен рабочий стол.
//! Запуск: `cargo test -p macshell --test icon_compare -- --ignored --nocapture`, пишет `icon_compare.bmp` во временную папку.

use glass::Bitmap;
use macshell::icons::{IconPixels, hicon_to_pixels, resample};
use macshell::snapshot::write_bmp;
use windows::Win32::UI::WindowsAndMessaging::{IDI_INFORMATION, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW};

fn put(canvas: &mut Bitmap, icon: &IconPixels, ox: u32, oy: u32, zoom: u32) {
    for y in 0..icon.height {
        for x in 0..icon.width {
            let i = ((y * icon.width + x) * 4) as usize;
            let (r, g, b, a) = (icon.rgba[i] as u32, icon.rgba[i + 1] as u32, icon.rgba[i + 2] as u32, icon.rgba[i + 3] as u32);
            // Поверх серого фона 60, как тёмное стекло.
            let over = |c: u32| (c * a + 60 * (255 - a)) / 255;
            let px = 0xFF000000 | over(r) << 16 | over(g) << 8 | over(b);
            for dy in 0..zoom {
                for dx in 0..zoom {
                    canvas.pixels[((oy + y * zoom + dy) * canvas.width + ox + x * zoom + dx) as usize] = px;
                }
            }
        }
    }
}

/// Растяжка «ближайший сосед»: так ступеньками увеличивается значок 16 x 16 без нормального фильтра.
fn stretch_nearest(src: &IconPixels, target: u32) -> IconPixels {
    let mut rgba = Vec::new();
    for y in 0..target {
        for x in 0..target {
            let (sx, sy) = (x * src.width / target, y * src.height / target);
            let i = ((sy * src.width + sx) * 4) as usize;
            rgba.extend_from_slice(&src.rgba[i..i + 4]);
        }
    }
    IconPixels { width: target, height: target, rgba }
}

#[test]
#[ignore]
fn write_stretch_versus_lanczos_comparison() {
    unsafe {
        // Исходный значок 16 x 16, как у большинства значков трея.
        let system = LoadImageW(None, IDI_INFORMATION, IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR | LR_SHARED).unwrap();
        let small = hicon_to_pixels(system.0 as isize, 16).unwrap();
        let old = stretch_nearest(&small, 24); // растяжка ступеньками
        let new = resample(&small, 24); // фильтр Ланцоша
        let mut canvas = Bitmap::new(24 * 8 * 2 + 30, 24 * 8 + 20);
        canvas.pixels.fill(0xFF3C3C3C);
        put(&mut canvas, &old, 10, 10, 8);
        put(&mut canvas, &new, 24 * 8 + 20, 10, 8);
        let path = std::env::temp_dir().join("icon_compare.bmp");
        write_bmp(path.to_str().unwrap(), &canvas).unwrap();
        println!("записано: {}", path.display());
    }
}
