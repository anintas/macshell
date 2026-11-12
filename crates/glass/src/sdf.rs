//! Форма стекла: расстояние со знаком до скруглённого прямоугольника.
//! Координаты в пикселях, ноль в левом верхнем углу фигуры `w` × `h`.

/// Расстояние со знаком (отрицательное внутри). Радиус зажимается в `min(w, h) / 2`.
pub fn rounded_rect_sdf(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let r = r.min(w.min(h) / 2.0).max(0.0);
    let px = (x - w / 2.0).abs();
    let py = (y - h / 2.0).abs();
    let qx = px - (w / 2.0 - r);
    let qy = py - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Единичный вектор наружу (центральные разности, шаг 0,5).
pub fn sdf_gradient(x: f32, y: f32, w: f32, h: f32, r: f32) -> (f32, f32) {
    const S: f32 = 0.5;
    let gx = rounded_rect_sdf(x + S, y, w, h, r) - rounded_rect_sdf(x - S, y, w, h, r);
    let gy = rounded_rect_sdf(x, y + S, w, h, r) - rounded_rect_sdf(x, y - S, w, h, r);
    let len = (gx * gx + gy * gy).sqrt();
    if len < 1e-6 { (0.0, 0.0) } else { (gx / len, gy / len) }
}

/// Длина смещения на кромке: `strength * (1 - d / bevel)^2` при `d = -sdf` в `[0, bevel]`, иначе 0.
pub fn edge_displacement(sdf: f32, bevel: f32, strength: f32) -> f32 {
    let d = -sdf;
    if bevel <= 0.0 || d < 0.0 || d > bevel {
        return 0.0;
    }
    let t = 1.0 - d / bevel;
    strength * t * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn sdf_center_of_stadium() {
        assert!(close(rounded_rect_sdf(50.0, 20.0, 100.0, 40.0, 20.0), -20.0));
    }

    #[test]
    fn sdf_outside_left() {
        assert!(close(rounded_rect_sdf(-5.0, 20.0, 100.0, 40.0, 20.0), 5.0));
    }

    #[test]
    fn sdf_corner_arc() {
        assert!(close(rounded_rect_sdf(0.0, 0.0, 100.0, 40.0, 20.0), 8.2843));
    }

    #[test]
    fn sdf_radius_is_clamped() {
        assert!(close(
            rounded_rect_sdf(50.0, 20.0, 100.0, 40.0, 100.0),
            rounded_rect_sdf(50.0, 20.0, 100.0, 40.0, 20.0)
        ));
    }

    #[test]
    fn gradient_points_outward_at_top_edge() {
        let (gx, gy) = sdf_gradient(50.0, 2.0, 100.0, 40.0, 20.0);
        assert!(close(gx, 0.0) && close(gy, -1.0));
    }

    #[test]
    fn displacement_profile() {
        assert!(close(edge_displacement(-1.0, 10.0, 8.0), 6.48));
        assert!(close(edge_displacement(-10.0, 10.0, 8.0), 0.0));
        assert!(close(edge_displacement(0.0, 10.0, 8.0), 8.0));
        assert!(close(edge_displacement(3.0, 10.0, 8.0), 0.0));
    }
}
