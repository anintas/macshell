//! Отладочный снимок: окна приложения исключены из захвата экрана, поэтому кадр для проверки глазами пишем сами.

use anyhow::Result;
use glass::Bitmap;

/// 32-битный BMP сверху вниз.
pub fn write_bmp(path: &str, b: &Bitmap) -> Result<()> {
    let data_len = b.pixels.len() * 4;
    let mut f = Vec::with_capacity(54 + data_len);
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&((54 + data_len) as u32).to_le_bytes());
    f.extend_from_slice(&[0; 4]);
    f.extend_from_slice(&54u32.to_le_bytes());
    f.extend_from_slice(&40u32.to_le_bytes());
    f.extend_from_slice(&(b.width as i32).to_le_bytes());
    f.extend_from_slice(&(-(b.height as i32)).to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&32u16.to_le_bytes());
    f.extend_from_slice(&[0; 24]);
    for p in &b.pixels {
        f.extend_from_slice(&p.to_le_bytes());
    }
    std::fs::write(path, f)?;
    Ok(())
}
