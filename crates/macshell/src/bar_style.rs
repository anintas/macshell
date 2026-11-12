//! Вид адаптивной панели: яркость обоев и цвет значков, сплошной фон, «окно закрывает рабочий стол».
//! Всё здесь чистые функции, окна системы читает `cover.rs`.

use capture::RectI;
use glass::Bitmap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    White,
    Black,
}

/// Порог цвета значков по яркости: выше чёрный, ниже белый, между сохраняется прежний (без дрожания).
const BLACK_ABOVE: f32 = 0.22;
const WHITE_BELOW: f32 = 0.14;

fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Средняя относительная яркость WCAG (0 чёрное, 1 белое) пикселей `0xAARRGGBB`.
pub fn relative_luminance(pixels: &[u32]) -> f32 {
    if pixels.is_empty() {
        return 0.0;
    }
    let table: Vec<f32> = (0..=255u8).map(srgb_to_linear).collect();
    let sum: f64 = pixels
        .iter()
        .map(|p| {
            let (r, g, b) = (table[(p >> 16) as usize & 0xFF], table[(p >> 8) as usize & 0xFF], table[*p as usize & 0xFF]);
            (0.2126 * r + 0.7152 * g + 0.0722 * b) as f64
        })
        .sum();
    (sum / pixels.len() as f64) as f32
}

pub fn choose_ink(luminance: f32, previous: Ink) -> Ink {
    if luminance > BLACK_ABOVE {
        Ink::Black
    } else if luminance < WHITE_BELOW {
        Ink::White
    } else {
        previous
    }
}

/// Умножает предумноженные пиксели на `factor` (0..1): так фон проявляется и гаснет без перерисовки.
pub fn scale_premultiplied(src: &[u32], factor: f32, out: &mut Vec<u32>) {
    out.clear();
    out.extend(src.iter().map(|p| {
        let ch = |shift: u32| (((p >> shift) & 0xFF) as f32 * factor).round().clamp(0.0, 255.0) as u32;
        ch(24) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0)
    }));
}

/// Сплошной непрозрачный фон панели: один цвет без градиента и линий, чтобы панель сливалась с чёлкой Island
/// (она чёрная, если в Island не выбран свой цвет).
pub fn solid_background(width: u32, height: u32, rgb: [u8; 3]) -> Bitmap {
    let px = 0xFF00_0000 | (rgb[0] as u32) << 16 | (rgb[1] as u32) << 8 | rgb[2] as u32;
    let mut b = Bitmap::new(width, height);
    b.pixels.fill(px);
    b
}

/// Пиксели столбцов от доли `from` до доли `to` ширины, строка за строкой: так берём левую и правую часть
/// полосы, чтобы цвет значков выбирался по обоям под каждой из них отдельно.
pub fn column_slice(b: &Bitmap, from: f32, to: f32) -> Vec<u32> {
    let x0 = (from * b.width as f32).round() as usize;
    let x1 = ((to * b.width as f32).round() as usize).min(b.width as usize);
    if x1 <= x0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity((x1 - x0) * b.height as usize);
    for row in b.pixels.chunks_exact(b.width as usize) {
        out.extend_from_slice(&row[x0..x1]);
    }
    out
}

/// Мягкое размытие для стекла всплывающих окон: три прохода прямоугольного размытия дают почти гауссово
/// пятно (как в macOS) вместо «квадратов» одного прохода. `radius`: радиус одного прохода, равного по силе
/// (дисперсия трёх проходов радиуса r/√3 примерно равна дисперсии одного радиуса r).
pub fn soft_blur(src: &Bitmap, radius: u32) -> Bitmap {
    if radius == 0 {
        return src.clone();
    }
    let pass = ((radius as f32 / 3f32.sqrt()).round() as u32).max(1);
    let once = glass::cpu::box_blur(src, pass);
    let twice = glass::cpu::box_blur(&once, pass);
    glass::cpu::box_blur(&twice, pass)
}

/// Размытый снимок экрана для крупного стекла (Spotlight). Размытие большим радиусом считается на копии,
/// уменьшенной в `factor` раз, и увеличивается билинейно только в нужной области: на глаз то же самое, потому что
/// размытие и так убирает мелкие детали, а считается в десятки раз быстрее, чем `soft_blur` на полном размере.
pub struct SoftBackdrop {
    small: Bitmap,
    factor: u32,
}

/// Во сколько раз уменьшается снимок перед размытием.
const BACKDROP_FACTOR: u32 = 4;

fn downscale(src: &Bitmap, f: u32) -> Bitmap {
    let (sw, sh) = (src.width.div_ceil(f), src.height.div_ceil(f));
    let mut out = Bitmap::new(sw, sh);
    for by in 0..sh {
        for bx in 0..sw {
            let (mut sum, mut n) = ([0u32; 4], 0u32);
            for y in by * f..((by + 1) * f).min(src.height) {
                let row = (y * src.width) as usize;
                for x in bx * f..((bx + 1) * f).min(src.width) {
                    let p = src.pixels[row + x as usize];
                    sum[0] += p >> 24;
                    sum[1] += (p >> 16) & 0xFF;
                    sum[2] += (p >> 8) & 0xFF;
                    sum[3] += p & 0xFF;
                    n += 1;
                }
            }
            let avg = |v: u32| (v + n / 2) / n;
            out.pixels[(by * sw + bx) as usize] = avg(sum[0]) << 24 | avg(sum[1]) << 16 | avg(sum[2]) << 8 | avg(sum[3]);
        }
    }
    out
}

/// Смесь двух пикселей `0xAARRGGBB`: `t` от 0 до 256 доля `b`. Четыре канала считаются парами в двух умножениях.
fn lerp_pixel(a: u32, b: u32, t: u32) -> u32 {
    let rb = (((a & 0x00FF_00FF) * (256 - t) + (b & 0x00FF_00FF) * t) >> 8) & 0x00FF_00FF;
    let ag = (((a >> 8) & 0x00FF_00FF) * (256 - t) + ((b >> 8) & 0x00FF_00FF) * t) & 0xFF00_FF00;
    rb | ag
}

impl SoftBackdrop {
    pub fn new(shot: &Bitmap, radius: u32) -> SoftBackdrop {
        let f = BACKDROP_FACTOR;
        if radius < 2 * f || shot.width < 2 * f || shot.height < 2 * f {
            return SoftBackdrop { small: soft_blur(shot, radius), factor: 1 };
        }
        SoftBackdrop { small: soft_blur(&downscale(shot, f), (radius / f).max(1)), factor: f }
    }

    /// Размытый фон в прямоугольнике `w` x `h` от точки (`x`, `y`) исходного снимка.
    pub fn region(&self, x: u32, y: u32, w: u32, h: u32) -> Bitmap {
        if self.factor == 1 {
            return crop(&self.small, x, y, w, h);
        }
        let f = self.factor as f32;
        let (sw, sh) = (self.small.width as usize, self.small.height as usize);
        let axis = |origin: u32, i: u32, len: usize| -> (usize, usize, u32) {
            let s = (((origin + i) as f32 + 0.5) / f - 0.5).clamp(0.0, (len - 1) as f32);
            let a = s.floor() as usize;
            (a, (a + 1).min(len - 1), ((s - a as f32) * 256.0) as u32)
        };
        let cols: Vec<(usize, usize, u32)> = (0..w).map(|i| axis(x, i, sw)).collect();
        let mut out = Bitmap::new(w, h);
        for j in 0..h {
            let (y0, y1, fy) = axis(y, j, sh);
            let (r0, r1) = (&self.small.pixels[y0 * sw..(y0 + 1) * sw], &self.small.pixels[y1 * sw..(y1 + 1) * sw]);
            let dst = &mut out.pixels[(j * w) as usize..((j + 1) * w) as usize];
            for (d, &(x0, x1, fx)) in dst.iter_mut().zip(&cols) {
                *d = lerp_pixel(lerp_pixel(r0[x0], r0[x1], fx), lerp_pixel(r1[x0], r1[x1], fx), fy);
            }
        }
        out
    }
}

/// Прямоугольник `rect`, расширенный на `margin` во все стороны и обрезанный экраном `screen_w` x `screen_h`.
/// Фон снимается с запасом: размытие у кромки окна берёт настоящие соседние пиксели, а не повторяет крайние.
pub fn padded_rect(rect: RectI, margin: i32, screen_w: i32, screen_h: i32) -> RectI {
    let x0 = (rect.x - margin).max(0);
    let y0 = (rect.y - margin).max(0);
    let x1 = (rect.x + rect.w + margin).min(screen_w.max(rect.x + rect.w));
    let y1 = (rect.y + rect.h + margin).min(screen_h.max(rect.y + rect.h));
    RectI { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

/// Часть картинки: `w` x `h` от точки (`x`, `y`). Точка и размер должны лежать внутри `b`.
pub fn crop(b: &Bitmap, x: u32, y: u32, w: u32, h: u32) -> Bitmap {
    debug_assert!(x + w <= b.width && y + h <= b.height);
    let mut out = Bitmap::new(w, h);
    for row in 0..h {
        let from = ((y + row) * b.width + x) as usize;
        out.pixels[(row * w) as usize..((row + 1) * w) as usize].copy_from_slice(&b.pixels[from..from + w as usize]);
    }
    out
}

/// Что известно об окне верхнего уровня для решения «закрывает ли оно рабочий стол».
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WinInfo {
    pub rect: RectI,
    pub maximized: bool,
    pub minimized: bool,
    pub visible: bool,
    /// Скрыто системой (окна UWP на других рабочих столах и т.п.): на экране его нет.
    pub cloaked: bool,
    pub tool_window: bool,
    /// Прозрачное для мыши наложение (`WS_EX_LAYERED | WS_EX_TRANSPARENT`): оверлеи игр и видеокарты,
    /// затемнители экрана, записи экрана. Рабочий стол под ним виден и доступен, это не окно приложения.
    pub click_through: bool,
    /// Рабочий стол, панель задач, наши окна: не приложение.
    pub shell_class: bool,
}

/// Окно пропускает мышь насквозь: `WS_EX_LAYERED` (0x80000) вместе с `WS_EX_TRANSPARENT` (0x20).
pub fn is_click_through(ex_style: u32) -> bool {
    const LAYERED: u32 = 0x0008_0000;
    const TRANSPARENT: u32 = 0x0000_0020;
    ex_style & (LAYERED | TRANSPARENT) == LAYERED | TRANSPARENT
}

fn intersection_area(a: RectI, b: RectI) -> i64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0 || h <= 0 { 0 } else { w as i64 * h as i64 }
}

/// Закрывает ли окно рабочий стол основного монитора: развёрнуто (и лежит на этом мониторе) или занимает
/// почти всю рабочую область.
pub fn covers(w: &WinInfo, work: RectI) -> bool {
    if !w.visible || w.minimized || w.cloaked || w.tool_window || w.click_through || w.shell_class {
        return false;
    }
    let share = intersection_area(w.rect, work) as f64 / (work.w as i64 * work.h as i64).max(1) as f64;
    if w.maximized { share >= 0.5 } else { share >= 0.95 }
}

/// Рабочий стол не виден, если его закрывает хоть одно окно. Порядок окон не важен: окно под другим
/// всё равно закрывает рабочий стол.
pub fn desktop_covered(windows: &[WinInfo], work: RectI) -> bool {
    windows.iter().any(|w| covers(w, work))
}

/// Задевает ли какое-то окно приложения прямоугольник Dock `band`: тогда Dock уходит под нижний край.
/// Окна оболочки, прозрачные наложения, свёрнутые и скрытые системой Dock не трогают.
pub fn dock_overlapped(windows: &[WinInfo], band: RectI) -> bool {
    windows.iter().any(|w| {
        !(!w.visible || w.minimized || w.cloaked || w.tool_window || w.click_through || w.shell_class)
            && intersection_area(w.rect, band) > 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> RectI {
        RectI { x, y, w, h }
    }

    const WORK: RectI = RectI { x: 0, y: 24, w: 1920, h: 1056 };

    fn win(r: RectI) -> WinInfo {
        WinInfo {
            rect: r,
            maximized: false,
            minimized: false,
            visible: true,
            cloaked: false,
            tool_window: false,
            click_through: false,
            shell_class: false,
        }
    }

    #[test]
    fn click_through_overlay_does_not_cover() {
        // Оверлей на весь экран (игровой счётчик кадров, затемнитель) не делает панель сплошной.
        let overlay = WinInfo { click_through: true, ..win(rect(0, 0, 1920, 1080)) };
        assert!(!covers(&overlay, WORK));
        assert!(!desktop_covered(&[overlay.clone()], WORK));
        let maximized = WinInfo { maximized: true, ..win(rect(-8, 16, 1936, 1072)) };
        assert!(desktop_covered(&[overlay, maximized], WORK));
    }

    #[test]
    fn click_through_needs_both_layered_and_transparent() {
        assert!(is_click_through(0x0008_0000 | 0x0000_0020));
        assert!(!is_click_through(0x0008_0000), "layered only: ordinary window with alpha");
        assert!(!is_click_through(0x0000_0020), "transparent only: hit testing still normal");
    }

    #[test]
    fn black_wallpaper_gets_white_ink() {
        assert_eq!(choose_ink(relative_luminance(&[0xFF000000; 64]), Ink::Black), Ink::White);
    }

    #[test]
    fn white_wallpaper_gets_black_ink() {
        assert_eq!(choose_ink(relative_luminance(&[0xFFFFFFFF; 64]), Ink::White), Ink::Black);
    }

    #[test]
    fn midtone_keeps_previous_ink() {
        let gray = relative_luminance(&[0xFF808080; 64]);
        assert_eq!(choose_ink(gray, Ink::White), Ink::White);
        assert_eq!(choose_ink(gray, Ink::Black), Ink::Black);
    }

    #[test]
    fn empty_region_has_zero_luminance() {
        assert_eq!(relative_luminance(&[]), 0.0);
    }

    #[test]
    fn scale_premultiplied_halves_all_channels() {
        let mut out = Vec::new();
        scale_premultiplied(&[0xFF804020, 0x00000000], 0.5, &mut out);
        assert_eq!(out, vec![0x80402010, 0x00000000]);
        scale_premultiplied(&[0xFF804020], 0.0, &mut out);
        assert_eq!(out, vec![0]);
        scale_premultiplied(&[0xFF804020], 1.0, &mut out);
        assert_eq!(out, vec![0xFF804020]);
    }

    #[test]
    fn solid_background_is_one_opaque_colour() {
        let b = solid_background(40, 24, [0x10, 0x20, 0x30]);
        assert_eq!((b.width, b.height), (40, 24));
        assert!(b.pixels.iter().all(|&p| p == 0xFF10_2030), "flat, opaque, no hairlines");
        assert!(solid_background(2, 2, [0, 0, 0]).pixels.iter().all(|&p| p == 0xFF00_0000));
    }

    #[test]
    fn maximized_window_covers() {
        let w = WinInfo { maximized: true, ..win(rect(-8, 16, 1936, 1072)) };
        assert!(covers(&w, WORK));
    }

    #[test]
    fn near_full_size_window_covers() {
        assert!(covers(&win(rect(0, 0, 1920, 1080)), WORK));
        assert!(covers(&win(rect(0, 24, 1900, 1040)), WORK));
    }

    #[test]
    fn small_minimized_cloaked_tool_shell_or_other_monitor_windows_do_not_cover() {
        assert!(!covers(&win(rect(200, 200, 800, 600)), WORK));
        assert!(!covers(&WinInfo { minimized: true, ..win(rect(0, 0, 1920, 1080)) }, WORK));
        assert!(!covers(&WinInfo { cloaked: true, ..win(rect(0, 0, 1920, 1080)) }, WORK));
        assert!(!covers(&WinInfo { visible: false, ..win(rect(0, 0, 1920, 1080)) }, WORK));
        assert!(!covers(&WinInfo { tool_window: true, ..win(rect(0, 0, 1920, 1080)) }, WORK));
        assert!(!covers(&WinInfo { shell_class: true, ..win(rect(0, 0, 1920, 1080)) }, WORK));
        let other_monitor = WinInfo { maximized: true, ..win(rect(1920, 0, 1920, 1080)) };
        assert!(!covers(&other_monitor, WORK));
    }

    #[test]
    fn dock_hides_only_under_app_windows_that_reach_it() {
        let band = rect(700, 1000, 520, 72);
        let reaching = win(rect(100, 100, 1700, 950)); // нижний край на 1050: заходит в Dock
        let above = win(rect(100, 100, 1700, 880)); // нижний край на 980
        let beside = win(rect(0, 900, 600, 180)); // левее подложки Dock
        assert!(dock_overlapped(&[reaching.clone()], band));
        assert!(!dock_overlapped(&[above, beside], band));
        for ignored in [
            WinInfo { minimized: true, ..reaching.clone() },
            WinInfo { cloaked: true, ..reaching.clone() },
            WinInfo { tool_window: true, ..reaching.clone() },
            WinInfo { click_through: true, ..reaching.clone() },
            WinInfo { shell_class: true, ..reaching.clone() },
            WinInfo { visible: false, ..reaching },
        ] {
            assert!(!dock_overlapped(&[ignored], band));
        }
        assert!(!dock_overlapped(&[], band));
    }

    #[test]
    fn covering_window_behind_another_still_covers() {
        let small_on_top = win(rect(300, 300, 400, 300));
        let maximized_behind = WinInfo { maximized: true, ..win(rect(-8, 16, 1936, 1072)) };
        assert!(desktop_covered(&[small_on_top.clone(), maximized_behind], WORK));
        assert!(!desktop_covered(&[small_on_top], WORK));
        assert!(!desktop_covered(&[], WORK));
    }
}

#[cfg(test)]
mod popup_glass_tests {
    use super::*;

    fn red(p: u32) -> u32 {
        (p >> 16) & 0xFF
    }

    #[test]
    fn soft_blur_falls_off_smoothly_unlike_a_single_box() {
        // Белая точка на чёрном: у одного прохода вокруг неё плато (все соседи равны), у трёх спад от центра.
        let n = 41u32;
        let mut b = Bitmap::new(n, n);
        b.pixels.fill(0xFF000000);
        b.pixels[(20 * n + 20) as usize] = 0xFFFFFFFF;
        let soft = soft_blur(&b, 6);
        let row: Vec<u32> = (20..30).map(|x| red(soft.pixels[(20 * n + x) as usize])).collect();
        assert!(row.windows(2).all(|w| w[0] >= w[1]), "no rise away from the center: {row:?}");
        assert!(row[0] > row[3] && row[3] > row[6], "smooth falloff: {row:?}");
        let boxed = glass::cpu::box_blur(&b, 6);
        assert_eq!(red(boxed.pixels[(20 * n + 20) as usize]), red(boxed.pixels[(20 * n + 23) as usize]), "box plateau");
        assert!(soft_blur(&b, 0) == b);
    }

    #[test]
    fn padded_rect_grows_and_stays_on_screen() {
        let r = RectI { x: 100, y: 30, w: 200, h: 100 };
        assert_eq!(padded_rect(r, 20, 1920, 1080), RectI { x: 80, y: 10, w: 240, h: 140 });
        let corner = RectI { x: 5, y: 0, w: 50, h: 50 };
        assert_eq!(padded_rect(corner, 20, 1920, 1080), RectI { x: 0, y: 0, w: 75, h: 70 });
        let right = RectI { x: 1900, y: 1060, w: 20, h: 20 };
        assert_eq!(padded_rect(right, 20, 1920, 1080), RectI { x: 1880, y: 1040, w: 40, h: 40 });
    }

    #[test]
    fn crop_takes_the_inner_block() {
        let mut b = Bitmap::new(4, 3);
        for (i, p) in b.pixels.iter_mut().enumerate() {
            *p = i as u32;
        }
        let c = crop(&b, 1, 1, 2, 2);
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.pixels, vec![5, 6, 9, 10]);
    }
}

#[cfg(test)]
mod backdrop_tests {
    use super::*;

    /// Полосы с резкими краями и шум: плохой случай для размытия на уменьшенной копии.
    fn busy(w: u32, h: u32) -> Bitmap {
        let mut b = Bitmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if (x / 13 + y / 7) % 2 == 0 { 230 } else { 20 } + ((x * 7 + y * 13) % 17);
                b.pixels[(y * w + x) as usize] = 0xFF000000 | v << 16 | (v / 2) << 8 | (255 - v);
            }
        }
        b
    }

    #[test]
    fn lerp_pixel_mixes_all_four_channels() {
        assert_eq!(lerp_pixel(0xFF00_FF00, 0x0000_00FF, 0), 0xFF00_FF00);
        assert_eq!(lerp_pixel(0xFF00_FF00, 0x0000_00FF, 256), 0x0000_00FF);
        assert_eq!(lerp_pixel(0xFF00_0000, 0x0000_0000, 128), 0x7F00_0000);
    }

    #[test]
    fn flat_picture_stays_flat() {
        let mut flat = Bitmap::new(200, 120);
        flat.pixels.fill(0xFF304050);
        let region = SoftBackdrop::new(&flat, 24).region(10, 5, 150, 90);
        assert_eq!((region.width, region.height), (150, 90));
        assert!(region.pixels.iter().all(|&p| p == 0xFF304050), "flat picture changed");
    }

    #[test]
    fn matches_the_full_size_blur_closely() {
        let shot = busy(240, 160);
        let radius = 24;
        let exact = soft_blur(&shot, radius);
        let fast = SoftBackdrop::new(&shot, radius).region(0, 0, 240, 160);
        // Средняя разница по каналам внутри (у самой кромки размытие по-разному берёт соседей).
        let (mut diff, mut n) = (0u64, 0u64);
        for y in 30..130u32 {
            for x in 30..210u32 {
                let (a, b) = (exact.pixels[(y * 240 + x) as usize], fast.pixels[(y * 240 + x) as usize]);
                for shift in [16, 8, 0] {
                    diff += (((a >> shift) & 0xFF) as i64 - ((b >> shift) & 0xFF) as i64).unsigned_abs();
                    n += 1;
                }
            }
        }
        let mean = diff as f64 / n as f64;
        assert!(mean < 6.0, "mean channel difference {mean:.2} of 255");
    }

    #[test]
    fn small_radius_falls_back_to_the_exact_blur() {
        let shot = busy(60, 40);
        let exact = soft_blur(&shot, 4);
        let fast = SoftBackdrop::new(&shot, 4).region(0, 0, 60, 40);
        assert_eq!(exact.pixels, fast.pixels);
    }

    #[test]
    fn region_at_the_far_edge_does_not_overrun() {
        let shot = busy(97, 53); // размеры не делятся на 4
        let backdrop = SoftBackdrop::new(&shot, 20);
        let r = backdrop.region(40, 20, 57, 33);
        assert_eq!((r.width, r.height), (57, 33));
    }
}

#[cfg(test)]
mod zone_tests {
    use super::*;

    #[test]
    fn column_slice_takes_the_requested_fraction_of_every_row() {
        let mut b = Bitmap::new(10, 2);
        for y in 0..2u32 {
            for x in 0..10u32 {
                b.pixels[(y * 10 + x) as usize] = x; // номер столбца в пикселе
            }
        }
        let left: Vec<u32> = column_slice(&b, 0.0, 0.3);
        assert_eq!(left, vec![0, 1, 2, 0, 1, 2]);
        let right = column_slice(&b, 0.8, 1.0);
        assert_eq!(right, vec![8, 9, 8, 9]);
        assert!(column_slice(&b, 0.5, 0.5).is_empty());
    }
}
