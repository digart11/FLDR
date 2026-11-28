//! Single-owner address row paint shared by breadcrumbs and the inline editor.
use crate::ui::{
    address_policy::{self, FieldState},
    surface,
};
use gpui::{point, px, Bounds, Entity, Hsla, IntoElement, Pixels, RenderOnce, Styled, Window};
use gpui_component::input::InputState;

pub(crate) fn darker(material: Hsla, reduction: f32) -> Hsla {
    let c = gpui::hsla_to_rgba(material);
    let [r, g, b, a] =
        address_policy::darker_material([c.red, c.green, c.blue, c.alpha], reduction);
    gpui::rgb_to_hsla(gpui::Rgba::new(r, g, b, a))
}
fn rounded(path: &mut gpui::PathBuilder, b: Bounds<Pixels>, radius: f32) {
    let r = radius
        .min(f32::from(b.size.width.min(b.size.height)) * 0.5)
        .max(0.0);
    let x = f32::from(b.left());
    let y = f32::from(b.top());
    let right = f32::from(b.right());
    let bottom = f32::from(b.bottom());
    let k = r * 0.5522848;
    path.move_to(point(px(x + r), px(y)));
    path.line_to(point(px(right - r), px(y)));
    path.cubic_bezier_to(
        point(px(right), px(y + r)),
        point(px(right - r + k), px(y)),
        point(px(right), px(y + r - k)),
    );
    path.line_to(point(px(right), px(bottom - r)));
    path.cubic_bezier_to(
        point(px(right - r), px(bottom)),
        point(px(right), px(bottom - r + k)),
        point(px(right - r + k), px(bottom)),
    );
    path.line_to(point(px(x + r), px(bottom)));
    path.cubic_bezier_to(
        point(px(x), px(bottom - r)),
        point(px(x + r - k), px(bottom)),
        point(px(x), px(bottom - r + k)),
    );
    path.line_to(point(px(x), px(y + r)));
    path.cubic_bezier_to(
        point(px(x + r), px(y)),
        point(px(x), px(y + r - k)),
        point(px(x + r - k), px(y)),
    );
    path.close();
}
pub(crate) fn paint(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    field: Option<Bounds<Pixels>>,
    radius: f32,
    navigation: Hsla,
    ink: Hsla,
    state: FieldState,
) {
    let Some(field) = field.filter(|b| b.size.width > px(0.) && b.size.height > px(0.)) else {
        surface::paint_fill(window, bounds, navigation);
        return;
    };
    // Use identical curves for the hole and replacement, never navigation under
    // the field. Thus the shared Glass pane backing receives one local coat.
    let mut outside = gpui::PathBuilder::fill().with_style(gpui::PathStyle::Fill(
        gpui::FillOptions::default().with_fill_rule(gpui::FillRule::EvenOdd),
    ));
    outside.add_polygon(
        &[
            bounds.origin,
            point(bounds.right(), bounds.top()),
            point(bounds.right(), bounds.bottom()),
            point(bounds.left(), bounds.bottom()),
        ],
        true,
    );
    rounded(&mut outside, field, radius);
    if let Ok(path) = outside.build() {
        window.paint_path(path, navigation);
    }
    let mut inside = gpui::PathBuilder::fill();
    rounded(&mut inside, field, radius);
    if let Ok(path) = inside.build() {
        window.paint_path(path, darker(navigation, 0.03));
    }
    let alpha = address_policy::outline_alpha(state);
    if alpha > 0.0 {
        let mut edge = gpui::PathBuilder::stroke(px(1.0 / window.scale_factor()));
        rounded(&mut edge, field, radius);
        let mut color = ink;
        color.alpha = alpha;
        if let Ok(path) = edge.build() {
            window.paint_path(path, color);
        }
    }
}
pub(crate) fn height(window: &Window, windows: bool) -> f32 {
    let zoom = f32::from(window.rem_size()) / crate::ui::BASE_PX;
    let font_size = surface::chrome_font_size_crumbs(window, windows);
    let line = surface::shape_text(
        "Hg",
        &surface::chrome_ui_font(window),
        font_size,
        gpui::black(),
        window,
    );
    address_policy::field_height(
        if windows { 38.0 } else { 32.0 } * zoom,
        f32::from(line.ascent + line.descent).max(f32::from(font_size) * 1.25),
        window.scale_factor(),
    )
}
#[derive(IntoElement)]
pub(crate) struct EditorInput {
    pub state: Entity<InputState>,
    pub windows: bool,
}
impl RenderOnce for EditorInput {
    fn render(self, window: &mut Window, _: &mut gpui::App) -> impl IntoElement {
        // Preserve the native editor's original 32-rem-pixel pointer target.
        // Its text and the smaller painted well share the same row center.
        let hit_height = 32.0 * f32::from(window.rem_size()) / crate::ui::BASE_PX;
        let font = surface::chrome_font_size_crumbs(window, self.windows);
        gpui::Styled::h(crate::ui::text_input::flat(&self.state), px(hit_height))
            .py_0()
            .px(crate::ui::rem_px(if self.windows { 13.0 } else { 11.0 }))
            .text_size(font)
            .line_height(font * 1.25)
    }
}
