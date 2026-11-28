//! Shared logical caption geometry, rounded outwards at the physical clip edge.
pub(crate) const CONTROL_HEIGHT: f32 = 30.;
pub(crate) const CONTROL_WIDTH: f32 = 92.;
pub(crate) const CONTROL_INSET: f32 = 16.;
pub(crate) const CONTROL_GAP: f32 = 4.;

pub(crate) fn caption_top(row_height: f32, inset: f32, zoom: f32) -> f32 {
    inset + ((row_height - CONTROL_HEIGHT * zoom) * 0.5).max(0.)
}

pub(crate) fn available_width(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    viewport_width: f32,
    controls_top: f32,
    zoom: f32,
    scale: f32,
) -> f32 {
    if top >= controls_top + CONTROL_HEIGHT * zoom || top + height <= controls_top {
        return width;
    }
    let edge = viewport_width - (CONTROL_INSET + CONTROL_WIDTH + CONTROL_GAP) * zoom;
    let edge = (edge * scale).floor() / scale;
    width.min((edge - left).max(0.))
}
