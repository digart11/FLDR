//! Pure local surface/geometry policy; no global theme or input changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldState {
    Idle,
    Hovered,
    Focused,
}
pub(crate) fn outline_alpha(state: FieldState) -> f32 {
    match state {
        FieldState::Idle => 0.0,
        FieldState::Hovered => 0.12,
        FieldState::Focused => 0.30,
    }
}
pub(crate) fn inactive_reduction(hover: f32) -> f32 {
    let hover = if hover.is_finite() {
        hover.clamp(0.0, 1.0)
    } else {
        0.0
    };
    // One-third recession separates low-alpha Glass tabs without another coat.
    // Hover restores five points while remaining clearly below active material.
    0.35 - 0.05 * hover
}
/// Replacement-over-B = factor * (original-over-B), for any backing B.
/// Fold darkening into one material instead of adding a translucent coat.
pub(crate) fn darker_material(mut rgba: [f32; 4], reduction: f32) -> [f32; 4] {
    let factor = 1.0 - reduction.clamp(0.0, 1.0);
    let alpha = 1.0 - factor * (1.0 - rgba[3]);
    let ratio = if alpha > 0.0 {
        factor * rgba[3] / alpha
    } else {
        0.0
    };
    for channel in &mut rgba[..3] {
        *channel *= ratio;
    }
    rgba[3] = alpha;
    rgba
}
/// Three physical pixels smaller, retaining four physical pixels around text.
pub(crate) fn field_height(original: f32, text_height: f32, scale: f32) -> f32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let spare = (original - text_height - 4.0 / scale).max(0.0);
    original - (3.0 / scale).min(spare)
}
pub(crate) fn centered_y(top: f32, height: f32, content: f32) -> f32 {
    top + (height - content) * 0.5
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_material_darkens_once_over_dark_light_and_unknown_glass_backings() {
        for color in [
            [0.9, 0.8, 0.7, 0.035],
            [0.1, 0.2, 0.3, 0.7],
            [0.5, 0.4, 0.8, 1.0],
        ] {
            for drop in [0.03, 0.20, 0.25] {
                let dim = darker_material(color, drop);
                for backing in [0.02, 0.3, 0.95] {
                    for channel in 0..3 {
                        let active = color[channel] * color[3] + backing * (1.0 - color[3]);
                        let inactive = dim[channel] * dim[3] + backing * (1.0 - dim[3]);
                        assert!((inactive - active * (1.0 - drop)).abs() < 0.00001);
                    }
                }
            }
        }
    }
    #[test]
    fn address_state_uses_only_temporary_outlines() {
        assert_eq!(outline_alpha(FieldState::Idle), 0.0);
        assert!(outline_alpha(FieldState::Focused) > outline_alpha(FieldState::Hovered));
    }
    #[test]
    fn inactive_tabs_keep_thirty_to_thirty_five_percent_contrast_even_on_hover() {
        assert_eq!(inactive_reduction(0.0), 0.35);
        for hover in [-1.0, 0.0, 0.5, 1.0, 2.0, f32::NAN] {
            assert!((0.2999..=0.35).contains(&inactive_reduction(hover)));
        }
    }
    #[test]
    fn field_reduction_and_centering_follow_dpi_zoom_and_font_height() {
        for zoom in [0.75, 1.0, 1.5, 2.0] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                for original in [32.0 * zoom, 38.0 * zoom] {
                    let text = 18.0 * zoom;
                    let height = field_height(original, text, scale);
                    assert!(((original - height) * scale - 3.0).abs() < 0.00001);
                    assert!(height >= text + 4.0 / scale);
                    let top = centered_y(0.0, 44.0 * zoom, height);
                    assert!(
                        (centered_y(top, height, text) + text * 0.5 - 22.0 * zoom).abs() < 0.00001
                    );
                }
            }
        }
        assert_eq!(field_height(24.0, 22.0, 1.0), 24.0);
        assert_eq!(field_height(24.0, 19.0, 1.0), 23.0);
    }
}
