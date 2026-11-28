//! Small input lifecycles shared by overlay controls; no layout or OS state.
/// Windows exposes independent horizontal/vertical drag distances in physical pixels.
pub(crate) fn drag_started(dx: f32, dy: f32, threshold: (f32, f32)) -> bool {
    dx.abs() >= threshold.0 || dy.abs() >= threshold.1
}

/// Clicks target the entire row; only the name/icon owns file dragging.
pub(crate) fn blank_row_marquee(row: Option<usize>, item: Option<usize>, checkbox: bool, clicks: usize) -> bool {
    row.is_some() && item.is_none() && !checkbox && clicks == 1
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct SliderCapture {
    pub id: &'static str,
    left: f32,
    width: f32,
    held: bool,
}
impl SliderCapture {
    pub fn new(id: &'static str, left: f32, width: f32) -> Self {
        Self {
            id,
            left,
            width,
            held: width.is_finite() && width > 0.0 && left.is_finite(),
        }
    }
    /// Keep the initial physical track stable while live zoom moves the panel.
    /// Once released, late pointer moves cannot revive this capture.
    pub fn sample(&mut self, x: f32, left_down: bool) -> Option<f32> {
        self.held &= left_down;
        (self.held && x.is_finite()).then(|| ((x - self.left) / self.width).clamp(0.0, 1.0))
    }
    pub fn finish(&mut self) {
        self.held = false;
    }
}

#[derive(Default)]
pub(crate) struct HoverLatch {
    pub epoch: u64,
    pub pinned: bool,
    pub inside: bool,
    pub interacting: bool,
}
impl HoverLatch {
    pub fn touch(&mut self) -> u64 {
        self.epoch = self.epoch.wrapping_add(1);
        self.epoch
    }
    pub fn should_close(&self, token: u64) -> bool {
        !self.pinned && !self.interacting && !self.inside && self.epoch == token
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn row_click_and_drag_targets_are_independent() {
        assert!(blank_row_marquee(Some(3), None, false, 1));
        assert!(!blank_row_marquee(Some(3), Some(3), false, 1));
        assert!(!blank_row_marquee(Some(3), None, true, 1));
        assert!(!blank_row_marquee(Some(3), None, false, 2));
        assert!(!blank_row_marquee(None, None, false, 1));
    }
    #[test]
    fn system_distance_uses_both_axes_without_diagonal_early_start() {
        assert!(!drag_started(3., 3., (4., 6.)));
        assert!(drag_started(-4., 0., (4., 6.)));
        assert!(!drag_started(0., 5., (4., 6.)));
        assert!(drag_started(0., -6., (4., 6.)));
        // Physical metrics converted at 150 percent retain the same boundary.
        assert!(!drag_started(3., 3., (6. / 1.5, 9. / 1.5)));
    }
    #[test]
    fn release_and_late_moves_cannot_reacquire_slider() {
        let mut capture = SliderCapture::new("zoom", 100., 200.);
        assert_eq!(capture.id, "zoom");
        assert_eq!(capture.sample(200., true), Some(0.5));
        assert_eq!(capture.sample(230., false), None);
        assert_eq!(capture.sample(240., true), None);
    }
    #[test]
    fn fixed_track_survives_layout_changes_and_duplicate_release() {
        let mut capture = SliderCapture::new("zoom", 100., 200.);
        assert_eq!(capture.sample(150., true), Some(0.25));
        // New zoom/layout bounds are deliberately not supplied to the gesture.
        assert_eq!(capture.sample(250., true), Some(0.75));
        capture.finish();
        capture.finish();
        assert_eq!(capture.sample(300., true), None);
        assert_eq!(SliderCapture::new("zoom", 0., 0.).sample(1., true), None);
    }
    #[test]
    fn entering_menu_invalidates_trigger_leave_and_click_pin_survives_leave() {
        let mut latch = HoverLatch::default();
        let leaving_trigger = latch.touch();
        latch.touch();
        assert!(!latch.should_close(leaving_trigger));
        let leaving_menu = latch.touch();
        assert!(latch.should_close(leaving_menu));
        latch.pinned = true;
        assert!(!latch.should_close(leaving_menu));
    }
    #[test]
    fn slider_interaction_blocks_hover_close_until_release_outside() {
        let mut latch = HoverLatch::default();
        latch.inside = true;
        latch.interacting = true;
        latch.inside = false;
        let token = latch.touch();
        assert!(!latch.should_close(token));
        latch.interacting = false;
        assert!(latch.should_close(token));
        latch.inside = true;
        assert!(!latch.should_close(token));
    }
}
