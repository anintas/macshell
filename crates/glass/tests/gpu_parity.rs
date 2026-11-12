//! GPU-путь против CPU-пути. Используется программный растеризатор WARP, видеокарта не нужна.

use glass::gpu::{GpuDriver, GpuGlass};
use glass::{Bitmap, GlassParams, cpu};

fn gradient_backdrop() -> Bitmap {
    let (w, h) = (120u32, 48u32);
    let mut b = Bitmap::new(w, h);
    for y in 0..h {
        for x in 0..w {
            b.pixels[(y * w + x) as usize] = 0xFF000000 | ((2 * x) << 16) | ((3 * y) << 8) | 128;
        }
    }
    b
}

fn params() -> GlassParams {
    GlassParams { blur_radius: 4, ..GlassParams::default() }
}

#[test]
fn shader_compiles() {
    assert!(GpuGlass::new(GpuDriver::Warp).is_ok());
}

#[test]
fn gpu_matches_cpu_within_tolerance() {
    let backdrop = gradient_backdrop();
    let gpu = GpuGlass::new(GpuDriver::Warp).unwrap().render(&backdrop, &params()).unwrap();
    let cpu_out = cpu::render(&backdrop, &params());
    assert_eq!(gpu.pixels.len(), cpu_out.pixels.len());

    let mut sum = [0u64; 3]; // B, G, R
    let mut max_alpha_diff = 0u32;
    for (g, c) in gpu.pixels.iter().zip(&cpu_out.pixels) {
        for (i, shift) in [0u32, 8, 16].into_iter().enumerate() {
            let (a, b) = ((g >> shift) & 0xFF, (c >> shift) & 0xFF);
            sum[i] += a.abs_diff(b) as u64;
        }
        max_alpha_diff = max_alpha_diff.max((g >> 24).abs_diff(c >> 24));
    }
    let n = gpu.pixels.len() as f64;
    let mean: Vec<f64> = sum.iter().map(|s| *s as f64 / n).collect();
    assert!(mean.iter().all(|m| *m <= 6.0), "mean abs diff B,G,R = {mean:?}");
    assert!(max_alpha_diff <= 2, "max alpha diff = {max_alpha_diff}");
}

#[test]
fn output_size_matches_input() {
    let out = GpuGlass::new(GpuDriver::Warp).unwrap().render(&gradient_backdrop(), &params()).unwrap();
    assert_eq!((out.width, out.height), (120, 48));
}
