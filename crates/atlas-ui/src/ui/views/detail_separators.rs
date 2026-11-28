//! Draw column structure once per surface, behind row selection/hover fills.
use crate::app::App;
use super::details::DetailsRowLayout;
use gpui::{point, px, size, Bounds, Pixels, WeakEntity, Window};
use gpui_component::ActiveTheme;

pub(crate) fn paint(app: &WeakEntity<App>, layout: &DetailsRowLayout, bounds: Bounds<Pixels>, window: &mut Window, cx: &gpui::App) {
    let Some(app) = app.upgrade() else { return; };
    let settings = &app.read(cx).state.settings;
    if !settings.detail_column_separators { return; }
    let strength = if settings.detail_separator_strength.is_finite() {
        settings.detail_separator_strength.clamp(0.0, 1.0)
    } else { 0.15 };
    if strength == 0.0 { return; }
    let mut color = cx.theme().border;
    color.alpha *= strength;
    let scale = window.scale_factor();
    let origin = f32::from(bounds.origin.x);
    let width = f32::from(bounds.size.width);
    // Metadata column starts include Name's right edge. Exclude the table's outer edges.
    for column in &layout.columns {
        if let Some(x) = crate::ui::appearance_policy::separator_x(origin, column.x, width, scale) {
            window.paint_quad(gpui::fill(Bounds::new(point(px(x), bounds.origin.y),
                size(px(1.0 / scale), bounds.size.height)), color));
        }
    }
}
