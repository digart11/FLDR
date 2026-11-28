//! Theme-relative translucent row striping and physical-pixel column boundaries.
/// Ink overlay remains visible on pure black, including translucent surfaces.
pub(crate) fn stripe_alpha(strength: f32) -> f32 {
    let strength = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 0.3 };
    if strength == 0.0 { 0.0 } else { 0.02 + strength * 0.12 }
}
/// Physical-pixel aligned interior column boundary, independent of column order.
pub(crate) fn separator_x(origin: f32, column_x: f32, width: f32, scale: f32) -> Option<f32> {
    if ![origin, column_x, width, scale].iter().all(|value| value.is_finite())
        || column_x <= 0.0 || column_x >= width || scale <= 0.0 { return None; }
    let x = ((origin + column_x) * scale).round() / scale;
    (x > origin && x + 1.0 / scale <= origin + width).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stripe_contrast_survives_black_dark_light_and_glass() {
        let alpha = stripe_alpha(0.3);
        for (surface, ink) in [(0., 1.), (0.08, 0.9), (1., 0.), (0.8, 0.1)] {
            let composite = surface * (1. - alpha) + ink * alpha;
            assert!((composite - surface).abs() > 0.025);
        }
        assert_eq!(stripe_alpha(0.), 0.);
        assert!(stripe_alpha(1.) < 0.2);
        assert_eq!(stripe_alpha(f32::NAN), alpha);
    }
    #[test]
    fn separator_boundaries_follow_width_order_and_display_scale() {
        assert_eq!(separator_x(10.0, 160.0, 500.0, 1.0), Some(170.0));
        assert_eq!(separator_x(10.0, 240.0, 500.0, 1.0), Some(250.0));
        assert_eq!(separator_x(10.2, 160.1, 500.0, 1.25), Some(170.4));
        for x in [0.0, 500.0, 700.0, f32::NAN] { assert!(separator_x(10.0, x, 500.0, 1.5).is_none()); }
    }

}
