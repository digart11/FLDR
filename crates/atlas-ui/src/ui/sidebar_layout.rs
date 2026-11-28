//! Layout ownership: natural fixed navigation, one flexible middle, fixed shelf.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SidebarLayout {
    pub shelf_height: f32,
    pub minimum_height: f32,
    pub overflow: bool,
}

pub(crate) fn sidebar_layout(count: usize, expanded: bool, available: f32, upper: f32) -> SidebarLayout {
    let compact = if count == 0 { 104.0 } else { 176.0 };
    // The shelf margin belongs to the fixed bottom region, not the middle viewport.
    let margin = 12.0;
    let middle_min = 64.0;
    let shelf_height = if expanded {
        (compact + 144.0f32).min(available - upper - middle_min - margin).max(compact + 48.0)
    } else { compact };
    let minimum_height = upper + middle_min + shelf_height + margin;
    SidebarLayout { shelf_height, minimum_height, overflow: minimum_height > available + 0.5 }
}

/// Logical section viewports share a bounded budget, preserving middle/shelf space.
/// Stored requests remain untouched when a smaller window temporarily clamps them.
pub(crate) fn navigation_heights(natural: [f32; 2], requested: [Option<f32>; 2], budget: f32) -> [f32; 2] {
    let minimum = 52.0;
    let wanted = std::array::from_fn::<_, 2, _>(|index| requested[index]
        .filter(|height| height.is_finite())
        .unwrap_or_else(|| if natural[index].is_finite() { natural[index] } else { minimum })
        .clamp(minimum, 2000.0));
    let extra = ((budget - minimum * 2.0).max(0.0))
        .min(wanted[0] + wanted[1] - minimum * 2.0);
    let total_extra = (wanted[0] + wanted[1] - minimum * 2.0).max(1.0);
    std::array::from_fn(|index| minimum + extra * (wanted[index] - minimum) / total_extra)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_heights_use_defaults_and_leave_reserved_space() {
        assert_eq!(navigation_heights([180., 100.], [None, None], 400.), [180., 100.]);
        assert_eq!(navigation_heights([180., 100.], [Some(220.), None], 400.), [220., 100.]);
        let constrained = navigation_heights([300., 200.], [Some(900.), Some(800.)], 240.);
        assert!((constrained.iter().sum::<f32>() - 240.).abs() < 0.01);
        assert!(constrained.iter().all(|height| *height >= 52.));
        assert_eq!(navigation_heights([f32::NAN, 100.], [Some(f32::NAN), None], 100.)[1], 52.);
    }
    #[test]
    fn middle_owns_remaining_space_and_expansion_only_reduces_middle() {
        let compact = sidebar_layout(4, false, 800.0, 400.0);
        let expanded = sidebar_layout(4, true, 800.0, 400.0);
        assert_eq!(compact.shelf_height, 176.0);
        assert_eq!(expanded.shelf_height, 320.0);
        assert!(!compact.overflow && !expanded.overflow);
        assert_eq!(expanded.minimum_height - compact.minimum_height, 144.0);
    }
    #[test]
    fn expansion_remains_visible_when_height_is_constrained() {
        let compact = sidebar_layout(4, false, 700.0, 400.0);
        let expanded = sidebar_layout(4, true, 700.0, 400.0);
        assert!(expanded.shelf_height >= compact.shelf_height + 48.0);
        assert!(!expanded.overflow);
    }
    #[test]
    fn empty_shelf_is_usable_and_tiny_windows_get_explicit_fallback() {
        assert_eq!(sidebar_layout(0, false, 800.0, 400.0).shelf_height, 104.0);
        assert!(sidebar_layout(0, true, 800.0, 400.0).shelf_height > 104.0);
        let tiny = sidebar_layout(4, true, 400.0, 400.0);
        assert!(tiny.overflow && tiny.minimum_height > 400.0);
    }
}
