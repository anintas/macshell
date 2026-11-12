//! Значки файлов и картинок через оболочку Windows (`IShellItemImageFactory`): `.exe`, `.lnk`, `.ico`, `.png`
//! и всё, для чего у системы есть значок или миниатюра. Своего декодера картинок в приложении нет.

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HGDIOBJ,
    ReleaseDC,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT};
use windows::core::PCWSTR;

#[derive(Clone)]
/// Картинка без предумножения: `rgba` длиной `width * height * 4`, строки сверху вниз.
pub struct IconPixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Убирает часть слишком широких прозрачных полей, ограничивая увеличение 25%.
pub fn normalize_dock_icon(icon: &IconPixels) -> IconPixels {
    if icon.width != icon.height || icon.width == 0 || icon.rgba.len() != (icon.width * icon.height * 4) as usize { return icon.clone(); }
    let side = icon.width;
    let (mut left, mut top, mut right, mut bottom) = (side, side, 0, 0);
    let mut found = false;
    for y in 0..side { for x in 0..side {
        if icon.rgba[((y * side + x) * 4 + 3) as usize] >= 32 {
            left = left.min(x); top = top.min(y); right = right.max(x); bottom = bottom.max(y); found = true;
        }
    }}
    if !found { return icon.clone(); }
    let needed = (right - left + 1).max(bottom - top + 1) + 2;
    let crop_size = needed.max((side * 4 + 4) / 5).min(side);
    if crop_size == side { return icon.clone(); }
    let x0 = ((left + right + 1).saturating_sub(crop_size) / 2).min(side - crop_size);
    let y0 = ((top + bottom + 1).saturating_sub(crop_size) / 2).min(side - crop_size);
    let mut rgba = Vec::with_capacity((crop_size * crop_size * 4) as usize);
    for y in y0..y0 + crop_size {
        let start = ((y * side + x0) * 4) as usize;
        rgba.extend_from_slice(&icon.rgba[start..start + crop_size as usize * 4]);
    }
    IconPixels { width: crop_size, height: crop_size, rgba }
}

/// Из BGRA с предумноженной альфой в RGBA. Если альфы нет ни у одного пикселя (так отдают миниатюры
/// некоторых форматов), картинка считается непрозрачной.
fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let opaque = bgra.chunks_exact(4).all(|p| p[3] == 0);
    let mut out = Vec::with_capacity(bgra.len());
    for p in bgra.chunks_exact(4) {
        let a = if opaque { 255 } else { p[3] as u32 };
        let un = |v: u8| if a == 0 || opaque { v } else { ((v as u32 * 255 + a / 2) / a).min(255) as u8 };
        out.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a as u8]);
    }
    out
}

/// Значок или миниатюра файла размером до `px` пикселей. `None`, если файла нет или системе нечего показать.
pub fn load_icon(path: &str, px: i32) -> Option<IconPixels> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
        let hbmp = factory.GetImage(SIZE { cx: px, cy: px }, SIIGBF_RESIZETOFIT).ok()?;

        let mut bm = BITMAP::default();
        if GetObjectW(HGDIOBJ(hbmp.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbmp, 0, h as u32, Some(bgra.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        if lines == 0 {
            return None;
        }
        Some(IconPixels { width: w as u32, height: h as u32, rgba: bgra_to_rgba(&bgra) })
    }
}

/// Значок трея (`HICON` из `Shell_NotifyIcon`, 32-битный дескриптор со знаком) в пиксели `px` x `px`.
/// Альфа восстанавливается двойной отрисовкой на чёрном и белом фоне: так работают и значки с альфа-каналом,
/// и старые значки с маской.
pub fn hicon_to_pixels(hicon: isize, px: i32) -> Option<IconPixels> {
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, CreateDIBSection, DeleteDC, GdiFlush, SelectObject};
    use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, HICON};

    if hicon == 0 || px <= 0 {
        return None;
    }
    unsafe {
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: px,
                biHeight: -px,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        let Some(dib) = dib.filter(|_| !bits.is_null()) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let old = SelectObject(dc, HGDIOBJ(dib.0));
        let buf = std::slice::from_raw_parts_mut(bits as *mut u32, (px * px) as usize);
        let mut render = |fill: u32| -> Option<Vec<u32>> {
            buf.fill(fill);
            let ok = DrawIconEx(dc, 0, 0, HICON(hicon as *mut _), px, px, 0, None, DI_NORMAL).is_ok();
            let _ = GdiFlush();
            ok.then(|| buf.to_vec())
        };
        let (black, white) = (render(0xFF000000), render(0xFFFFFFFF));
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(dc);

        let (black, white) = (black?, white?);
        let mut rgba = Vec::with_capacity(black.len() * 4);
        for (b, w) in black.iter().zip(&white) {
            let ch = |shift: u32| ((b >> shift) & 0xFF, (w >> shift) & 0xFF);
            let alpha_of = |(b, w): (u32, u32)| 255 - w.saturating_sub(b).min(255);
            let (r, g, bl) = (ch(16), ch(8), ch(0));
            let a = (alpha_of(r) + alpha_of(g) + alpha_of(bl)) / 3;
            let un = |(b, _): (u32, u32)| if a == 0 { 0 } else { (b * 255 / a).min(255) as u8 };
            rgba.extend_from_slice(&[un(r), un(g), un(bl), a as u8]);
        }
        Some(IconPixels { width: px as u32, height: px as u32, rgba })
    }
}

fn lanczos3(x: f32) -> f32 {
    if x == 0.0 {
        return 1.0;
    }
    if x.abs() >= 3.0 {
        return 0.0;
    }
    let px = std::f32::consts::PI * x;
    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
}

/// Веса ядра Ланцоша для каждого выходного отсчёта: (первый входной индекс, веса). При уменьшении ядро
/// растягивается, чтобы усреднять все исходные пиксели, а не пропускать их.
fn lanczos_weights(src: u32, dst: u32) -> Vec<(i32, Vec<f32>)> {
    let ratio = src as f32 / dst as f32;
    let scale = ratio.max(1.0);
    let support = 3.0 * scale;
    (0..dst)
        .map(|i| {
            let center = (i as f32 + 0.5) * ratio - 0.5;
            let first = (center - support).ceil() as i32;
            let last = (center + support).floor() as i32;
            let mut w: Vec<f32> = (first..=last).map(|j| lanczos3((j as f32 - center) / scale)).collect();
            let sum: f32 = w.iter().sum();
            if sum.abs() > 1e-6 {
                w.iter_mut().for_each(|v| *v /= sum);
            }
            (first, w)
        })
        .collect()
}

/// Масштабирует значок до `target` x `target` фильтром Ланцоша в предумноженном виде (без тёмной каймы
/// у прозрачных краёв). Значки трея приходят в 16 или 32 пикселях: `DrawIconEx` растягивает их ступеньками,
/// здесь получается плавно и чётко.
pub fn resample(src: &IconPixels, target: u32) -> IconPixels {
    if src.width == target && src.height == target {
        return IconPixels { width: src.width, height: src.height, rgba: src.rgba.clone() };
    }
    let (sw, sh, t) = (src.width as usize, src.height as usize, target as usize);
    // Предумноженные каналы в f32.
    let pre: Vec<[f32; 4]> = src
        .rgba
        .chunks_exact(4)
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
        })
        .collect();
    let xs = lanczos_weights(src.width, target);
    let ys = lanczos_weights(src.height, target);
    // По горизонтали: sh строк по t столбцов.
    let mut mid = vec![[0.0f32; 4]; sh * t];
    for y in 0..sh {
        for (x, (first, w)) in xs.iter().enumerate() {
            let mut acc = [0.0f32; 4];
            for (k, wk) in w.iter().enumerate() {
                let sx = (*first + k as i32).clamp(0, sw as i32 - 1) as usize;
                let p = pre[y * sw + sx];
                for c in 0..4 {
                    acc[c] += p[c] * wk;
                }
            }
            mid[y * t + x] = acc;
        }
    }
    // По вертикали и обратно в непредумноженный RGBA.
    let mut rgba = Vec::with_capacity(t * t * 4);
    for (first, w) in &ys {
        for x in 0..t {
            let mut acc = [0.0f32; 4];
            for (k, wk) in w.iter().enumerate() {
                let sy = (*first + k as i32).clamp(0, sh as i32 - 1) as usize;
                let p = mid[sy * t + x];
                for c in 0..4 {
                    acc[c] += p[c] * wk;
                }
            }
            let a = acc[3].clamp(0.0, 255.0);
            let un = |v: f32| if a < 0.5 { 0 } else { (v * 255.0 / a).round().clamp(0.0, 255.0) as u8 };
            rgba.extend_from_slice(&[un(acc[0]), un(acc[1]), un(acc[2]), a.round() as u8]);
        }
    }
    IconPixels { width: target, height: target, rgba }
}

/// Размер самого изображения значка в пикселях (у значков трея чаще всего 16 или 32).
pub fn hicon_native_size(hicon: isize) -> Option<(u32, u32)> {
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, HICON, ICONINFO};
    unsafe {
        let mut info = ICONINFO::default();
        GetIconInfo(HICON(hicon as *mut _), &mut info).ok()?;
        let mut bm = BITMAP::default();
        // Цветной значок: размер у цветной картинки; чёрно-белый: маска вдвое выше значка.
        let size = if !info.hbmColor.is_invalid() {
            (GetObjectW(HGDIOBJ(info.hbmColor.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) != 0)
                .then_some((bm.bmWidth as u32, bm.bmHeight as u32))
        } else {
            (GetObjectW(HGDIOBJ(info.hbmMask.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) != 0)
                .then_some((bm.bmWidth as u32, bm.bmHeight as u32 / 2))
        };
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        }
        size.filter(|&(w, h)| w > 0 && h > 0)
    }
}

/// Значок трея размером `target`: рисуется в своём родном размере (без растяжки ступеньками) и затем
/// масштабируется фильтром Ланцоша.
pub fn hicon_to_pixels_hq(hicon: isize, target: u32) -> Option<IconPixels> {
    let native = hicon_native_size(hicon).map(|(w, h)| w.max(h)).unwrap_or(target);
    let base = hicon_to_pixels(hicon, native as i32)?;
    Some(resample(&base, target))
}

/// Шаблонный вид, как у значков строки меню macOS: белый цвет, форма из альфы, яркие места плотнее.
pub fn to_mono(icon: &IconPixels) -> IconPixels {
    let mut rgba = Vec::with_capacity(icon.rgba.len());
    for p in icon.rgba.chunks_exact(4) {
        let luma = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0;
        let alpha = (p[3] as f32 * (0.35 + 0.65 * luma)).round() as u8;
        rgba.extend_from_slice(&[255, 255, 255, alpha]);
    }
    IconPixels { width: icon.width, height: icon.height, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn dump_dock_icon_comparison() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let paths = [format!(r"{root}\explorer.exe"), format!(r"{root}\System32\notepad.exe"), format!(r"{root}\System32\cmd.exe"), format!(r"{root}\System32\mspaint.exe")];
        let mut image = glass::Bitmap::new(4 * 80, 2 * 80);
        image.pixels.fill(0xff292c33);
        for (col, path) in paths.iter().enumerate() {
            let Some(original) = load_icon(path, 64) else { continue };
            for (row, icon) in [original.clone(), normalize_dock_icon(&original)].iter().enumerate() {
                let icon = resample(icon, 64);
                for y in 0..64usize { for x in 0..64usize {
                    let i = (y * 64 + x) * 4;
                    let alpha = icon.rgba[i + 3] as u32;
                    let blend = |v: u8, back: u32| (v as u32 * alpha + back * (255 - alpha)) / 255;
                    let pixel = 0xff000000 | blend(icon.rgba[i], 41) << 16 | blend(icon.rgba[i + 1], 44) << 8 | blend(icon.rgba[i + 2], 51);
                    image.pixels[(row * 80 + y + 8) * image.width as usize + col * 80 + x + 8] = pixel;
                }}
            }
        }
        let dir = std::env::var("MACSHELL_DUMP_DIR").unwrap();
        crate::snapshot::write_bmp(&format!("{dir}\\dock_icon_comparison.bmp"), &image).unwrap();
    }

    #[test]
    fn dock_normalization_reduces_large_transparent_padding_without_changing_full_icons() {
        let mut icon = IconPixels { width: 100, height: 100, rgba: vec![0; 40000] };
        for y in 25..75 { for x in 25..75 { icon.rgba[(y * 100 + x) * 4 + 3] = 255; } }
        let normalized = normalize_dock_icon(&icon);
        assert!(normalized.width < 100 && normalized.width >= 80);
        let solid = IconPixels { width: 100, height: 100, rgba: vec![255; 40000] };
        assert_eq!(normalize_dock_icon(&solid).width, 100);
    }

    #[test]
    fn unpremultiply_restores_color() {
        // BGRA с альфой 128: 64 / 128 = 0.5 от полной яркости.
        assert_eq!(bgra_to_rgba(&[64, 32, 128, 128]), vec![255, 64, 128, 128]);
    }

    #[test]
    fn opaque_when_no_alpha_channel() {
        assert_eq!(bgra_to_rgba(&[10, 20, 30, 0]), vec![30, 20, 10, 255]);
    }

    #[test]
    fn loads_notepad_icon() {
        let path = format!("{}\\System32\\notepad.exe", std::env::var("SystemRoot").unwrap());
        let icon = load_icon(&path, 32).expect("notepad icon");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
        assert!(icon.rgba.chunks(4).any(|p| p[3] > 0));
    }

    #[test]
    fn missing_file_gives_none() {
        assert!(load_icon(r"C:\no\such\dir\file.exe", 32).is_none());
    }
}

#[cfg(test)]
mod tray_icon_tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{IDI_INFORMATION, LoadIconW};

    #[test]
    fn stock_icon_has_alpha_and_color() {
        let hicon = unsafe { LoadIconW(None, IDI_INFORMATION) }.unwrap();
        let icon = hicon_to_pixels(hicon.0 as isize, 32).expect("pixels");
        assert_eq!((icon.width, icon.height), (32, 32));
        let px: Vec<&[u8]> = icon.rgba.chunks(4).collect();
        assert!(px.iter().any(|p| p[3] == 0), "corners must be transparent");
        assert!(px.iter().any(|p| p[3] == 255 && (p[0] != p[1] || p[1] != p[2])), "opaque colored pixel expected");
    }

    #[test]
    fn mono_is_white_and_keeps_shape() {
        let icon = IconPixels {
            width: 3,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 0, 0, 20, 20, 20, 255],
        };
        let m = to_mono(&icon);
        assert_eq!(&m.rgba[0..3], &[255, 255, 255]);
        assert_eq!(m.rgba[7], 0, "transparent stays transparent");
        let (bright, dark) = (m.rgba[3], m.rgba[11]);
        assert!(bright > dark && dark > 0, "bright {bright} dark {dark}");
    }
}

#[cfg(test)]
mod resample_tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> IconPixels {
        IconPixels { width: w, height: h, rgba: rgba.iter().copied().cycle().take((w * h * 4) as usize).collect() }
    }

    fn px(icon: &IconPixels, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * icon.width + x) * 4) as usize;
        [icon.rgba[i], icon.rgba[i + 1], icon.rgba[i + 2], icon.rgba[i + 3]]
    }

    #[test]
    fn resize_keeps_a_constant_color_both_ways() {
        let up = resample(&solid(4, 4, [200, 100, 50, 255]), 12);
        assert_eq!((up.width, up.height), (12, 12));
        assert!(up.rgba.chunks(4).all(|p| p.iter().zip([200u8, 100, 50, 255]).all(|(a, b)| a.abs_diff(b) <= 1)));
        let down = resample(&up, 4);
        assert_eq!((down.width, down.height), (4, 4));
        assert!(down.rgba.chunks(4).all(|p| p.iter().zip([200u8, 100, 50, 255]).all(|(a, b)| a.abs_diff(b) <= 1)));
    }

    /// Левая половина белая непрозрачная, правая прозрачная чёрная.
    fn half_white(size: u32) -> IconPixels {
        let mut icon = solid(size, size, [0, 0, 0, 0]);
        for y in 0..size {
            for x in 0..size / 2 {
                let i = ((y * size + x) * 4) as usize;
                icon.rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        icon
    }

    #[test]
    fn upscaling_an_edge_is_smooth_not_blocky() {
        let up = resample(&half_white(8), 24);
        let row: Vec<u8> = (0..24).map(|x| px(&up, x, 12)[3]).collect();
        assert_eq!((row[0], row[23]), (255, 0));
        let in_between = row.iter().filter(|&&a| a > 10 && a < 245).count();
        assert!(in_between >= 2, "ребро должно переходить плавно, а не ступенькой: {row:?}");
    }

    #[test]
    fn transparent_neighbours_do_not_darken_the_edge() {
        let up = resample(&half_white(8), 24);
        for x in 0..24 {
            let p = px(&up, x, 12);
            if p[3] > 20 {
                assert!(p[0] > 235, "у ребра цвет должен оставаться белым, а не серым: x={x} {p:?}");
            }
        }
    }

    #[test]
    fn stock_icon_native_size_is_reported_and_hq_render_has_the_requested_size() {
        use windows::Win32::UI::WindowsAndMessaging::{IDI_INFORMATION, LoadIconW};
        let hicon = unsafe { LoadIconW(None, IDI_INFORMATION) }.unwrap();
        let (w, h) = hicon_native_size(hicon.0 as isize).expect("size");
        assert!(w >= 16 && w == h, "{w}x{h}");
        let icon = hicon_to_pixels_hq(hicon.0 as isize, 48).expect("pixels");
        assert_eq!((icon.width, icon.height), (48, 48));
        let px: Vec<&[u8]> = icon.rgba.chunks(4).collect();
        assert!(px.iter().any(|p| p[3] == 0) && px.iter().any(|p| p[3] == 255 && (p[0] != p[1] || p[1] != p[2])));
    }
}
