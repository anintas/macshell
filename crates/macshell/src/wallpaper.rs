//! Обои основного монитора как картинка размером с экран. Виджетам на рабочем столе нужен фон под стеклом, а снимок
//! экрана захватил бы окна приложений, лежащие сверху. Файл обоев декодирует WIC, раскладка (заполнение, по размеру,
//! растянуть, по центру) берётся из реестра. Результат кэшируется и перечитывается, когда файл обоев изменился.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use glass::Bitmap;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::{PCWSTR, w};

/// Как обои лежат на экране.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Закрывает экран целиком, лишнее обрезается.
    Fill,
    /// Помещается целиком, по краям чёрные поля.
    Contain,
    Stretch,
    Center,
}

/// `WallpaperStyle` из реестра: 10 заполнение, 6 по размеру, 2 растянуть, 0 по центру (или плитка), 22 на все экраны.
pub fn fit_from_style(style: &str) -> Fit {
    match style.trim() {
        "6" => Fit::Contain,
        "2" => Fit::Stretch,
        "0" => Fit::Center,
        _ => Fit::Fill,
    }
}

/// Размер и положение картинки `iw` x `ih` на экране `sw` x `sh`: (ширина, высота, сдвиг по x, сдвиг по y).
pub fn layout(fit: Fit, iw: u32, ih: u32, sw: u32, sh: u32) -> (u32, u32, i32, i32) {
    let (iw, ih, sw, sh) = (iw.max(1), ih.max(1), sw.max(1), sh.max(1));
    let (w, h) = match fit {
        Fit::Stretch => (sw, sh),
        Fit::Center => (iw, ih),
        Fit::Fill | Fit::Contain => {
            let (kx, ky) = (sw as f32 / iw as f32, sh as f32 / ih as f32);
            let k = if fit == Fit::Fill { kx.max(ky) } else { kx.min(ky) };
            (((iw as f32 * k).round() as u32).max(1), ((ih as f32 * k).round() as u32).max(1))
        }
    };
    (w, h, (sw as i32 - w as i32) / 2, (sh as i32 - h as i32) / 2)
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join(r"Microsoft\Windows\Themes\TranscodedWallpaper"))
}

fn style() -> String {
    let mut buf = [0u16; 8];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let read = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Control Panel\\Desktop"), w!("WallpaperStyle"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size))
    };
    if read.is_err() {
        return String::new();
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn decode(file: &std::path::Path, sw: u32, sh: u32, fit: Fit) -> Option<Bitmap> {
    use std::os::windows::ffi::OsStrExt;
    let name: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let decoder = factory.CreateDecoderFromFilename(PCWSTR(name.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut iw, mut ih) = (0u32, 0u32);
        frame.GetSize(&mut iw, &mut ih).ok()?;
        let converter = factory.CreateFormatConverter().ok()?;
        converter.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (dw, dh, ox, oy) = layout(fit, iw, ih, sw, sh);
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler.Initialize(&converter, dw, dh, WICBitmapInterpolationModeFant).ok()?;
        let mut out = Bitmap::new(sw, sh);
        out.pixels.fill(0xFF000000);
        // Видимая часть масштабированной картинки: от `src` в картинке до `dst` на экране.
        let (src_x, dst_x) = ((-ox).max(0) as u32, ox.max(0) as u32);
        let (src_y, dst_y) = ((-oy).max(0) as u32, oy.max(0) as u32);
        let (w, h) = ((dw - src_x).min(sw - dst_x), (dh - src_y).min(sh - dst_y));
        let mut rows = vec![0u32; (w * h) as usize];
        let rect = WICRect { X: src_x as i32, Y: src_y as i32, Width: w as i32, Height: h as i32 };
        let bytes = std::slice::from_raw_parts_mut(rows.as_mut_ptr() as *mut u8, rows.len() * 4);
        scaler.CopyPixels(&rect, w * 4, bytes).ok()?;
        for row in 0..h {
            let from = (row * w) as usize;
            let to = ((dst_y + row) * sw + dst_x) as usize;
            for (d, s) in out.pixels[to..to + w as usize].iter_mut().zip(&rows[from..from + w as usize]) {
                *d = *s | 0xFF000000;
            }
        }
        Some(out)
    }
}

/// Кэш: время изменения файла, размер экрана, картинка и когда её брали в последний раз.
static CACHE: Mutex<Option<(SystemTime, u32, u32, Arc<Bitmap>, Instant)>> = Mutex::new(None);

/// Сколько кэш живёт без обращений: картинка на весь экран занимает 8 МБ, держать её постоянно незачем.
const KEEP: Duration = Duration::from_secs(15);

/// Обои на весь экран `sw` x `sh`; `None`, если файла нет или он не читается (слайд-шоу из папки, однотонный фон).
pub fn screen(sw: u32, sh: u32) -> Option<Arc<Bitmap>> {
    let file = path()?;
    let stamp = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, w, h, bmp, used)) = cache.as_mut() {
        if *t == stamp && (*w, *h) == (sw, sh) {
            *used = Instant::now();
            return Some(bmp.clone());
        }
    }
    let bmp = Arc::new(decode(&file, sw, sh, fit_from_style(&style()))?);
    *cache = Some((stamp, sw, sh, bmp.clone(), Instant::now()));
    Some(bmp)
}

/// Отпускает картинку, если к ней давно не обращались.
pub fn trim() {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.as_ref().is_some_and(|c| c.4.elapsed() >= KEEP) {
        *cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_map_to_fits() {
        assert_eq!(fit_from_style("10"), Fit::Fill);
        assert_eq!(fit_from_style("6"), Fit::Contain);
        assert_eq!(fit_from_style("2"), Fit::Stretch);
        assert_eq!(fit_from_style("0"), Fit::Center);
        assert_eq!(fit_from_style(""), Fit::Fill);
    }

    #[test]
    fn fill_covers_the_screen_and_crops_the_overflow() {
        // Широкая картинка на экране 16:9: по высоте впритык, по бокам лишнее с обеих сторон.
        let (w, h, ox, oy) = layout(Fit::Fill, 4000, 1000, 1920, 1080);
        assert_eq!((h, oy), (1080, 0));
        assert!(w > 1920 && ox < 0 && ox == (1920 - w as i32) / 2);
    }

    #[test]
    fn contain_leaves_bars_and_stretch_fills() {
        let (w, h, ox, oy) = layout(Fit::Contain, 1000, 1000, 1920, 1080);
        assert_eq!((w, h, ox, oy), (1080, 1080, 420, 0));
        assert_eq!(layout(Fit::Stretch, 10, 10, 1920, 1080), (1920, 1080, 0, 0));
        assert_eq!(layout(Fit::Center, 100, 50, 1920, 1080), (100, 50, 910, 515));
    }

    #[test]
    #[ignore = "needs a desktop wallpaper"]
    fn current_wallpaper_decodes_to_screen_size() {
        let b = screen(1920, 1080).expect("wallpaper");
        assert_eq!((b.width, b.height), (1920, 1080));
        assert!(b.pixels.iter().any(|p| *p != 0xFF000000));
    }
}
