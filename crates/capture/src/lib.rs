//! Снимок экрана под окном (GDI). Свои окна видны в чужих снимках экрана (Win+Shift+S, PrintScreen, запись
//! экрана) и пропадают только на время собственного снимка подложки, чтобы стекло не снимало само себя.

use std::sync::Mutex;

use anyhow::{Result, bail};
use glass::Bitmap;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

struct ScreenDc(HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe { ReleaseDC(None, self.0) };
    }
}

struct MemDc(HDC);
impl Drop for MemDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

struct Dib(HBITMAP);
impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0.0));
        }
    }
}

/// Окна, которые `capture_region` прячет от захвата на время снимка.
static OWN: Mutex<Vec<isize>> = Mutex::new(Vec::new());

fn own() -> std::sync::MutexGuard<'static, Vec<isize>> {
    OWN.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_affinity(windows: &[isize], excluded: bool) {
    let mode = if excluded { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    for h in windows {
        // Окно могло закрыться: тогда менять нечего.
        let _ = unsafe { SetWindowDisplayAffinity(HWND(*h as *mut _), mode) };
    }
}

/// Снимок области рабочего стола без своих окон (см. `exclude_from_capture`). Размер результата всегда
/// `r.w × r.h`; части за экраном остаются чёрными непрозрачными.
pub fn capture_region(r: RectI) -> Result<Bitmap> {
    let windows = own();
    set_affinity(&windows, true);
    let shot = capture_region_raw(r);
    set_affinity(&windows, false);
    shot
}

/// Снимок со всеми окнами, как его видит любая программа для скриншотов.
pub fn capture_region_raw(r: RectI) -> Result<Bitmap> {
    if r.w <= 0 || r.h <= 0 {
        bail!("capture size must be positive, got {}x{}", r.w, r.h);
    }
    unsafe {
        let screen = ScreenDc(GetDC(None));
        if screen.0.is_invalid() {
            bail!("GetDC(NULL) failed");
        }
        let mem = MemDc(CreateCompatibleDC(Some(screen.0)));
        if mem.0.is_invalid() {
            bail!("CreateCompatibleDC failed");
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: r.w,
                biHeight: -r.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = Dib(CreateDIBSection(Some(mem.0), &info, DIB_RGB_COLORS, &mut bits, None, 0)?);
        if bits.is_null() {
            bail!("CreateDIBSection returned no bits");
        }
        let count = (r.w as usize) * (r.h as usize);
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, count);
        px.fill(0xFF000000);

        let old = SelectObject(mem.0, HGDIOBJ(dib.0.0));
        let blit = BitBlt(mem.0, 0, 0, r.w, r.h, Some(screen.0), r.x, r.y, SRCCOPY | CAPTUREBLT);
        let _ = GdiFlush();
        SelectObject(mem.0, old);
        blit?;

        let pixels = px.iter().map(|p| p | 0xFF000000).collect();
        Ok(Bitmap { width: r.w as u32, height: r.h as u32, pixels })
    }
}

/// Регистрирует окно: пока идёт `capture_region`, оно не попадает в снимок (`WDA_EXCLUDEFROMCAPTURE`), в остальное
/// время его видят все программы для скриншотов.
pub fn exclude_from_capture(hwnd: isize) -> Result<()> {
    let mut windows = own();
    if !windows.contains(&hwnd) {
        windows.push(hwnd);
    }
    Ok(())
}

/// Окно закрывается: его больше не нужно прятать.
pub fn forget_window(hwnd: isize) {
    own().retain(|h| *h != hwnd);
}
