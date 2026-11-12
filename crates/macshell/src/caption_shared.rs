//! Small, per-window caption image shared with macshell's Genie renderer.
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE, UnmapViewOfFile};
use windows::core::PCWSTR;

pub const MAX_CAPTION_HEIGHT: usize = 256;
pub const MAX_CAPTION_WIDTH: usize = 8192;
pub const MAGIC: u32 = 0x4D434831;

#[repr(C)]
struct Header { sequence: AtomicU32, magic:u32, width:u32, height:u32 }

pub fn mapping_name(pid:u32, hwnd:isize, width:usize) -> String {
    format!("Local\\macshell.caption.v1.{pid:08X}.{:016X}.{width}", hwnd as u32 as u64)
}

pub struct CaptionMapping { handle:HANDLE, view:MEMORY_MAPPED_VIEW_ADDRESS, width:usize }
impl CaptionMapping {
    pub fn new(pid:u32, hwnd:isize, width:usize) -> Option<Self> {
        if pid == 0 || hwnd == 0 || width == 0 || width > MAX_CAPTION_WIDTH { return None; }
        let bytes = std::mem::size_of::<Header>().checked_add(width.checked_mul(MAX_CAPTION_HEIGHT)?.checked_mul(4)?)?;
        let name:Vec<u16> = mapping_name(pid, hwnd, width).encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, u32::try_from(bytes).ok()?, PCWSTR(name.as_ptr())).ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, bytes);
            if view.Value.is_null() { let _ = CloseHandle(handle); return None; }
            Some(Self { handle, view, width })
        }
    }

    pub fn publish(&self, height:usize, pixels:&[u32]) -> bool {
        if height == 0 || height > MAX_CAPTION_HEIGHT || pixels.len() != self.width * height { return false; }
        unsafe {
            let header = &mut *(self.view.Value as *mut Header);
            header.sequence.fetch_add(1, Ordering::AcqRel);
            header.magic = MAGIC;
            header.width = self.width as u32;
            header.height = height as u32;
            let dest = (self.view.Value as *mut u8).add(std::mem::size_of::<Header>()) as *mut u32;
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), dest, pixels.len());
            header.sequence.fetch_add(1, Ordering::Release);
        }
        true
    }
}
impl Drop for CaptionMapping {
    fn drop(&mut self) { unsafe { let _ = UnmapViewOfFile(self.view); let _ = CloseHandle(self.handle); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn name_and_size_limits_are_stable() {
        assert_eq!(mapping_name(42, 0x1234, 800), "Local\\macshell.caption.v1.0000002A.0000000000001234.800");
        assert!(CaptionMapping::new(0, 1, 1).is_none());
        assert!(CaptionMapping::new(1, 1, MAX_CAPTION_WIDTH + 1).is_none());
        let map = CaptionMapping::new(std::process::id(), 1, 8).expect("mapping");
        assert!(!map.publish(MAX_CAPTION_HEIGHT + 1, &[]));
        assert!(!map.publish(1, &[0; 7]));
        assert!(map.publish(1, &[0xff123456; 8]));
    }
}
