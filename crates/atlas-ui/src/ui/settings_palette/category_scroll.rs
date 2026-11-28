//! Horizontal wheel policy; GPUI retains item layout and reveal ownership.
pub(super) fn wheel_delta(x: f32, y: f32) -> f32 {
    if x != 0. { x } else { y }
}
pub(super) fn offset(current: f32, delta: f32, max: f32) -> f32 {
    (current + delta).clamp(-max.max(0.), 0.)
}
#[cfg(test)]
mod tests {
    #[test]
    fn wheels_trackpads_and_arrows_share_clamped_horizontal_offsets() {
        for max in [0., 120., 600.] {
            assert_eq!(super::offset(0., -1000., max), -max);
            assert_eq!(super::offset(-max, 1000., max), 0.);
        }
        assert_eq!(super::wheel_delta(-24., -60.), -24.);
        assert_eq!(super::wheel_delta(0., -60.), -60.);
        assert_eq!(super::offset(-100., -24., 600.), -124.);
    }
}
