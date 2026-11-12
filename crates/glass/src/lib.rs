//! Жидкое стекло: чистая математика и рендер. Пиксели: BGRA, предумноженная альфа, `0xAARRGGBB`.

pub mod cpu;
pub mod gpu;
pub mod sdf;

/// Картинка в предумноженном BGRA (`u32 = 0xAARRGGBB`).
#[derive(Clone, Debug, PartialEq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Bitmap {
    /// Полностью прозрачная картинка.
    pub fn new(width: u32, height: u32) -> Bitmap {
        Bitmap { width, height, pixels: vec![0; (width as usize) * (height as usize)] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassParams {
    pub radius: f32,
    pub bevel: f32,
    pub refraction: f32,
    pub blur_radius: u32,
    pub saturation: f32,
    /// RGB и сила смешивания.
    pub tint: [f32; 4],
    pub dispersion: f32,
    pub rim: f32,
    pub opacity: f32,
}

impl GlassParams {
    /// Параметры заданы в логических пикселях; при масштабе DPI `s` длины умножаются, остальное не меняется.
    pub fn scaled(&self, s: f32) -> GlassParams {
        GlassParams {
            radius: self.radius * s,
            bevel: self.bevel * s,
            refraction: self.refraction * s,
            blur_radius: (self.blur_radius as f32 * s).round() as u32,
            dispersion: self.dispersion * s,
            ..*self
        }
    }
}

impl Default for GlassParams {
    fn default() -> Self {
        GlassParams {
            radius: 18.0,
            bevel: 14.0,
            refraction: 9.0,
            blur_radius: 6,
            saturation: 1.35,
            tint: [1.0, 1.0, 1.0, 0.14],
            dispersion: 1.5,
            rim: 0.35,
            opacity: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_match_spec() {
        let p = GlassParams::default();
        assert_eq!(p.radius, 18.0);
        assert_eq!(p.blur_radius, 6);
        assert_eq!(p.tint, [1.0, 1.0, 1.0, 0.14]);
    }

    #[test]
    fn bitmap_new_is_transparent() {
        let b = Bitmap::new(4, 3);
        assert_eq!(b.pixels.len(), 12);
        assert!(b.pixels.iter().all(|&p| p == 0));
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn scaled_multiplies_lengths_only() {
        let p = GlassParams::default().scaled(2.0);
        assert_eq!((p.radius, p.bevel, p.refraction, p.blur_radius, p.dispersion), (36.0, 28.0, 18.0, 12, 3.0));
        assert_eq!((p.saturation, p.tint, p.rim, p.opacity), (1.35, [1.0, 1.0, 1.0, 0.14], 0.35, 1.0));
    }
}
