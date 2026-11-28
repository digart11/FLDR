//! Contiguous pages preserve tree indices for drag/reorder and keyboard focus.
/// Reserve pagers only when both fit beside the add button and a usable tab.
pub(crate) fn pagers_fit(available: f32, button: f32, gap: f32, zoom: f32) -> bool {
    available >= 2. * (button + gap) + 48. * zoom
}

pub(crate) fn add_button_fits(width: f32, padding: f32, button: f32, gap: f32) -> bool {
    width >= 2. * padding + button + gap + 1.
}
pub(crate) fn page_start(first: usize, count: usize, page_size: usize, active: usize, reveal_active: bool) -> usize {
    let page_size = page_size.max(1);
    let mut first = first.min(count.saturating_sub(1) / page_size * page_size);
    let active = active.min(count.saturating_sub(1));
    if reveal_active && (active < first || active >= first + page_size) {
        first = active / page_size * page_size;
    }
    first
}

/// Keep the add control adjacent to the final painted tab, including close animation.
pub(crate) fn new_tab_x(last_right: f32, gap: f32, maximum: f32) -> f32 {
    (last_right + gap).min(maximum).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn add_button_follows_tabs_in_wide_and_narrow_panes() {
        assert_eq!(new_tab_x(336.0, 8.0, 960.0), 344.0);
        assert_eq!(new_tab_x(336.0, 8.0, 340.0), 340.0);
        assert_eq!(new_tab_x(0.0, 8.0, -10.0), 0.0);
    }
    #[test]
    fn resizing_and_keyboard_focus_reveal_the_active_tab() {
        assert_eq!(page_start(0, 12, 3, 7, true), 6);
        assert_eq!(page_start(6, 12, 2, 8, true), 8);
        assert_eq!(page_start(8, 12, 2, 1, true), 0);
    }
    #[test]
    fn explicit_navigation_can_show_other_pages() {
        assert_eq!(page_start(6, 12, 3, 0, false), 6);
        assert_eq!(page_start(9, 4, 3, 0, false), 3);
        assert_eq!(page_start(9, 0, 0, 0, true), 0);
    }
}
