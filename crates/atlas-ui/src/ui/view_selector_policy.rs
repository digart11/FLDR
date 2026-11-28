//! Popup selection is discrete; its Grid-only slider never changes view mode.
use atlas_model::settings::ViewMode;

pub(crate) const MODES: [ViewMode; 5] = [
    ViewMode::Details,
    ViewMode::Grid,
    ViewMode::List,
    ViewMode::Miller,
    ViewMode::Tree,
];

pub(crate) fn grid_thumbnail_size(value: f32) -> f32 {
    64. + (value.round().clamp(3., 23.) - 3.) * 16.
}

pub(crate) fn keyboard_mode(current: &ViewMode, key: &str) -> Option<ViewMode> {
    let index = MODES.iter().position(|m| m == current)?;
    let index = match key {
        "up" => index.saturating_sub(1),
        "down" => (index + 1).min(MODES.len() - 1),
        "home" => 0,
        "end" => MODES.len() - 1,
        _ => return None,
    };
    Some(MODES[index].clone())
}
