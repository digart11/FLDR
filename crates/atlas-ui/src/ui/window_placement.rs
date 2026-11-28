//! Pure geometry policy, also testable with `rustc --test` without GPUI.
pub(crate) fn fit_to_monitor(saved: [f32; 4], monitor: [f32; 4]) -> Option<[f32; 4]> {
    if !saved.iter().chain(monitor.iter()).all(|v| v.is_finite())
        || saved[2] <= 0.0 || saved[3] <= 0.0 || monitor[2] <= 0.0 || monitor[3] <= 0.0 {
        return None;
    }
    let [mx, my, mw, mh] = monitor;
    let width = saved[2].max(640.0).min(mw);
    let height = saved[3].max(400.0).min(mh);
    Some([saved[0].clamp(mx, mx + mw - width), saved[1].clamp(my, my + mh - height), width, height])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_secondary_monitor_and_negative_coordinates() {
        let rect = [-1700.0, 70.0, 1200.0, 800.0];
        assert_eq!(fit_to_monitor(rect, [-1920.0, 0.0, 1920.0, 1080.0]), Some(rect));
    }
    #[test]
    fn disconnected_monitor_and_smaller_display_remain_reachable() {
        assert_eq!(fit_to_monitor([3000.0, -2000.0, 1800.0, 1200.0], [0.0, 0.0, 1280.0, 720.0]), Some([0.0, 0.0, 1280.0, 720.0]));
    }
    #[test]
    fn rejects_invalid_saved_data() {
        for rect in [[f32::NAN, 0.0, 800.0, 600.0], [0.0, 0.0, 0.0, 600.0], [0.0, 0.0, 800.0, f32::INFINITY]] {
            assert!(fit_to_monitor(rect, [0.0, 0.0, 1920.0, 1080.0]).is_none());
        }
    }
}
