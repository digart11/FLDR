//! Geometry shared by Details paint and input; values are window logical pixels.
pub(crate) const HEADER_HEIGHT: f32 = 24.0;

pub(crate) fn header_height(zoom: f32, scale: f32) -> f32 {
    (HEADER_HEIGHT * zoom * scale).round().max(1.0) / scale
}
pub(crate) fn divider(top: f32, height: f32, scale: f32, measured_text_height: f32, nudge: f32) -> (f32, f32) {
    let bottom = ((top + height) * scale).floor();
    // ShapedLine centers ascent+descent within the supplied line height.
    let text_bottom = top + nudge + (height + measured_text_height) * 0.5;
    let clear_text = (text_bottom * scale).ceil() + 1.0;
    let y = (bottom - 3.0).max(clear_text).min(bottom - 1.0);
    (y / scale, 1.0 / scale)
}
pub(crate) fn name_hit_end(name_x: f32, column_width: f32, measured: f32, padding: f32) -> f32 {
    name_x + (measured.max(0.0) + padding).min(column_width.max(0.0))
}
pub(crate) fn item_hit(
    entry: Option<usize>,
    x: f32,
    ranges: &[(usize, f32, f32)],
) -> Option<usize> {
    entry.filter(|entry| {
        ranges
            .iter()
            .any(|(index, left, right)| index == entry && x >= *left && x <= *right)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filename_icon_hit_excludes_row_whitespace_and_metadata_columns() {
        let end = name_hit_end(36., 200., 60., 4.);
        let ranges = [(7, 8., end)];
        assert_eq!(item_hit(Some(7), 10., &ranges), Some(7));
        assert_eq!(item_hit(Some(7), 85., &ranges), Some(7));
        assert_eq!(item_hit(Some(7), 140., &ranges), None);
        assert_eq!(item_hit(Some(7), 260., &ranges), None);
        assert_eq!(item_hit(Some(8), 85., &ranges), None);
        assert_eq!(item_hit(None, 85., &ranges), None);
        assert_eq!(name_hit_end(36., 80., 600., 4.), 116.);
    }
    #[test]
    fn header_and_divider_share_a_clean_physical_pixel_boundary() {
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            for zoom in [0.75, 1., 1.1, 1.25, 1.5, 2.] {
                let height = header_height(zoom, scale);
                assert!((height * scale - (height * scale).round()).abs() < 0.001);
                let (y, thickness) = divider(13.25, height, scale, 12. * zoom, -zoom);
                assert!((y * scale - (y * scale).round()).abs() < 0.001);
                assert!((thickness * scale - 1.).abs() < 0.001);
                assert!(y + thickness <= 13.25 + height);
                assert!(y >= 13.25 - zoom + (height + 12. * zoom) * 0.5);
            }
        }
    }
    #[test]
    fn divider_moves_one_physical_pixel_without_changing_row_spacing() {
        let h = header_height(1., 1.);
        assert_eq!(h, 24.);
        assert_eq!(divider(0., h, 1., 12., -1.), (21., 1.));
        // A tall user font takes precedence over the compact divider position.
        assert_eq!(divider(0., h, 1., 20., -1.), (22., 1.));
    }
}
