//! Жидкое стекло на процессоре. Пиксели: предумноженный BGRA, `0xAARRGGBB`.

use crate::sdf::{edge_displacement, rounded_rect_sdf, sdf_gradient};
use crate::{Bitmap, GlassParams};

/// Единичный вектор от фигуры к источнику света (сверху слева). Тот же в `glass.hlsl`.
const LIGHT: (f32, f32) = (-0.4472136, -0.8944272);

/// Прямоугольное размытие (два прохода) с зажимом по краям.
pub fn box_blur(src: &Bitmap, radius: u32) -> Bitmap {
    if radius == 0 || src.width == 0 || src.height == 0 {
        return src.clone();
    }
    let (w, h, r) = (src.width as usize, src.height as usize, radius as i64);
    let mut tmp = vec![0u32; w * h];
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        blur_line(&src.pixels, &mut tmp, y * w, 1, w, r);
    }
    for x in 0..w {
        blur_line(&tmp, &mut out, x, w, h, r);
    }
    Bitmap { width: src.width, height: src.height, pixels: out }
}

/// Одна строка или столбец: `n` элементов с шагом `stride`, начиная с `start`.
fn blur_line(src: &[u32], dst: &mut [u32], start: usize, stride: usize, n: usize, r: i64) {
    let at = |i: i64| src[start + (i.clamp(0, n as i64 - 1) as usize) * stride];
    let size = (2 * r + 1) as u32;
    let mut sum = [0u32; 4];
    for j in -r..=r {
        add(&mut sum, at(j), true);
    }
    for i in 0..n as i64 {
        let mut px = 0u32;
        for (c, s) in sum.iter().enumerate() {
            px |= ((s + size / 2) / size) << (24 - 8 * c);
        }
        dst[start + i as usize * stride] = px;
        add(&mut sum, at(i - r), false);
        add(&mut sum, at(i + r + 1), true);
    }
}

fn add(sum: &mut [u32; 4], px: u32, plus: bool) {
    for (c, s) in sum.iter_mut().enumerate() {
        let v = (px >> (24 - 8 * c)) & 0xFF;
        if plus { *s += v } else { *s -= v }
    }
}

/// Билинейная выборка RGB в точке (`sx`, `sy`); центр пикселя `(x, y)` лежит в `(x + 0.5, y + 0.5)`.
fn sample(img: &Bitmap, sx: f32, sy: f32) -> [f32; 3] {
    let (w, h) = (img.width as i32, img.height as i32);
    let fx = sx - 0.5;
    let fy = sy - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let px = |x: i32, y: i32| img.pixels[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    let mut rgb = [0.0f32; 3];
    for (dx, dy, wgt) in [
        (0, 0, (1.0 - tx) * (1.0 - ty)),
        (1, 0, tx * (1.0 - ty)),
        (0, 1, (1.0 - tx) * ty),
        (1, 1, tx * ty),
    ] {
        let p = px(x0 + dx, y0 + dy);
        rgb[0] += ((p >> 16) & 0xFF) as f32 * wgt;
        rgb[1] += ((p >> 8) & 0xFF) as f32 * wgt;
        rgb[2] += (p & 0xFF) as f32 * wgt;
    }
    rgb
}

/// Стекло поверх подложки: размер результата равен размеру `backdrop`.
pub fn render(backdrop: &Bitmap, p: &GlassParams) -> Bitmap {
    let blurred = box_blur(backdrop, p.blur_radius);
    let (wu, hu) = (backdrop.width, backdrop.height);
    let (w, h) = (wu as f32, hu as f32);
    let mut out = Bitmap::new(wu, hu);
    // Градиент нужен только у кромки: смещение и блик живут внутри этой полосы.
    let edge_band = p.bevel.max(1.5);
    for y in 0..hu {
        for x in 0..wu {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let sdf = rounded_rect_sdf(cx, cy, w, h, p.radius);
            let coverage = (0.5 - sdf).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let (mut gx, mut gy, mut disp) = (0.0, 0.0, 0.0);
            if -sdf <= edge_band {
                (gx, gy) = sdf_gradient(cx, cy, w, h, p.radius);
                disp = edge_displacement(sdf, p.bevel, p.refraction);
            }
            // Выборка сдвигается внутрь фигуры (против градиента).
            let (bx, by) = (cx - gx * disp, cy - gy * disp);
            let spread = if p.refraction > 0.0 { p.dispersion * disp / p.refraction } else { 0.0 };
            let r = sample(&blurred, bx - gx * spread, by - gy * spread)[0];
            let g = sample(&blurred, bx, by)[1];
            let b = sample(&blurred, bx + gx * spread, by + gy * spread)[2];
            let mut c = [r, g, b];

            let lum = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
            let ta = p.tint[3];
            for (i, v) in c.iter_mut().enumerate() {
                *v = lum + (*v - lum) * p.saturation;
                *v = *v * (1.0 - ta) + p.tint[i] * 255.0 * ta;
            }

            // Блик: белая кромка 1,5 px вглубь. Свет сверху слева: сильный блик на кромках,
            // обращённых к нему, слабый отсвет на противоположных.
            let facing = gx * LIGHT.0 + gy * LIGHT.1;
            let rim = p.rim * (1.0 - (-sdf) / 1.5).clamp(0.0, 1.0) * (facing.max(0.0) + 0.4 * (-facing).max(0.0)).min(1.0);
            for v in c.iter_mut() {
                *v = *v * (1.0 - rim) + 255.0 * rim;
            }

            let a = coverage * p.opacity;
            let ch = |v: f32| (v.clamp(0.0, 255.0) * a).round() as u32;
            let alpha = (a * 255.0).round() as u32;
            out.pixels[(y * wu + x) as usize] = (alpha << 24) | (ch(c[0]) << 16) | (ch(c[1]) << 8) | ch(c[2]);
        }
    }
    out
}

/// Накладывает предумноженный `src` того же размера на `dst`: `out = src + dst * (255 - src_a) / 255`.
pub fn composite_over(dst: &mut Bitmap, src: &[u32]) {
    debug_assert_eq!(dst.pixels.len(), src.len());
    for (d, &s) in dst.pixels.iter_mut().zip(src) {
        let inv = 255 - (s >> 24);
        let mut px = 0u32;
        for shift in [0u32, 8, 16, 24] {
            let sc = (s >> shift) & 0xFF;
            let dc = (*d >> shift) & 0xFF;
            px |= (sc + (dc * inv + 127) / 255).min(255) << shift;
        }
        *d = px;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: u32) -> Bitmap {
        Bitmap { width: w, height: h, pixels: vec![c; (w * h) as usize] }
    }

    fn red(p: u32) -> i32 {
        ((p >> 16) & 0xFF) as i32
    }

    /// Параметры без размытия, насыщенности, подкраски, блика и дисперсии.
    fn plain(radius: f32) -> GlassParams {
        GlassParams {
            radius,
            blur_radius: 0,
            saturation: 1.0,
            tint: [1.0, 1.0, 1.0, 0.0],
            rim: 0.0,
            dispersion: 0.0,
            ..GlassParams::default()
        }
    }

    #[test]
    fn box_blur_spreads_single_pixel() {
        let mut b = solid(5, 5, 0xFF000000);
        b.pixels[2 * 5 + 2] = 0xFFFFFFFF;
        let o = box_blur(&b, 1);
        assert!((red(o.pixels[2 * 5 + 2]) - 28).abs() <= 1);
        assert!((red(o.pixels[5 + 1]) - 28).abs() <= 1);
        assert_eq!(red(o.pixels[0]), 0);
    }

    #[test]
    fn outside_shape_is_transparent() {
        let b = solid(100, 40, 0xFF808080);
        let o = render(&b, &GlassParams { radius: 20.0, ..GlassParams::default() });
        assert_eq!(o.pixels[0] >> 24, 0);
    }

    #[test]
    fn center_is_opaque_and_unrefracted() {
        let b = solid(100, 40, 0xFF808080);
        let o = render(&b, &plain(20.0));
        assert_eq!(o.pixels[20 * 100 + 50], 0xFF808080);
    }

    #[test]
    fn left_edge_samples_from_inside() {
        let mut b = solid(100, 40, 0xFF000000);
        for y in 0..40usize {
            for x in 0..100usize {
                b.pixels[y * 100 + x] = 0xFF000000 | (((2 * x) as u32) << 16);
            }
        }
        let o = render(&b, &plain(18.0));
        assert!(red(o.pixels[20 * 100 + 3]) > 2 * 3);
    }

    #[test]
    fn rim_is_brighter_on_top_than_bottom() {
        let b = solid(100, 40, 0xFF404040);
        let o = render(&b, &GlassParams { rim: 1.0, ..plain(20.0) });
        let (top, bottom, center) = (red(o.pixels[50]), red(o.pixels[39 * 100 + 50]), red(o.pixels[20 * 100 + 50]));
        assert!(top > bottom && bottom > center, "top {top}, bottom {bottom}, center {center}");
    }

    #[test]
    fn composite_over_blends_premultiplied() {
        let mut dst = solid(1, 1, 0xFF0000FF);
        composite_over(&mut dst, &[0x80800000]);
        let p = dst.pixels[0];
        assert_eq!(red(p), 128);
        let blue = (p & 0xFF) as i32;
        assert!((126..=128).contains(&blue));
        assert_eq!(p >> 24, 255);
    }
}
