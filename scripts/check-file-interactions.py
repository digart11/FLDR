"""Source wiring checks only; no application compilation or runtime claims.

Run: py -3 scripts/check-file-interactions.py
Behavioral GPUI/transfer regressions live beside the production Rust code.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    return (ROOT / path).read_text(encoding="utf-8")


UI = "crates/atlas-ui/src/"
INTERACTIONS = source(UI + "ui/interactions.rs")
SIDEBAR = source(UI + "ui/sidebar.rs")
ACTIONS = source(UI + "app/actions.rs")
DRAG = source(UI + "ui/drag_hover.rs")
APP = source(UI + "app.rs")
DIV = source("vendor/gpui-ce/crates/gpui/src/elements/div.rs")


class InteractionWiring(unittest.TestCase):
    def test_item_press_is_not_a_background_marquee(self):
        self.assertIn("if click_count < 2 && (entry_idx.is_none() || selected_click)", INTERACTIONS)
        self.assertNotIn("(!selected && (entry_idx.is_none()", INTERACTIONS)
        diagnostic = source(UI + "ui/views/selection_diagnostic.rs")
        self.assertIn("first item drag was blocked", diagnostic)
        self.assertIn("replacement drag retained an unrelated group", diagnostic)

    def test_selected_group_uses_the_shared_drag_gate(self):
        self.assertIn("selected && !shift && !ctrl && click_count == 1", INTERACTIONS)
        self.assertIn("r.selected_click.is_some()) { self.rubber = None; }", INTERACTIONS)
        self.assertIn("app.prepare_selection_file_drag()", SIDEBAR)
        self.assertIn("app.prepare_selection_file_drag()", source(UI + "ui/drag.rs"))

    def test_background_retains_marquee_and_click_clear(self):
        self.assertIn("entry_idx.is_none() && !shift && !ctrl", INTERACTIONS)
        self.assertIn("RubberTarget::Dropzone => self.dropzone_selection.clear()", INTERACTIONS)
        self.assertIn("selection_left_mouse_down(crate::app::RubberTarget::Dropzone, None", SIDEBAR)

    def test_select_all_owns_only_the_clicked_surface(self):
        branch = ACTIONS.split("Action::SelectAll => {", 1)[1].split("Action::SetGroupBy", 1)[0]
        self.assertIn("self.dropzone_focused && self.state.settings.show_sidebar", branch)
        self.assertIn("sidebar_drop_zone.iter()", branch)
        self.assertIn("flat_visible_indices(tile)", branch)
        self.assertIn("self.dropzone_focused = false;", source(UI + "app/pane_ops.rs"))
        self.assertIn("capture_any_mouse_down", source(UI + "ui/root.rs"))

    def test_shelf_scroll_and_selection_share_content_coordinates(self):
        self.assertIn("RubberTarget::Dropzone => self.dropzone_scroll.clone()", INTERACTIONS)
        self.assertIn("matches!(target, RubberTarget::Dropzone) && !self.state.settings.drop_zone_expanded", INTERACTIONS)
        self.assertIn("RubberTarget::Dropzone => !self.state.sidebar_drop_zone.is_empty()", INTERACTIONS)
        self.assertIn("let origin = self.rubber_content_point(target, b.origin)", INTERACTIONS)
        self.assertIn("f32::from(r.content_origin.x)", INTERACTIONS)
        self.assertIn("this.dropzone_scroll = Some(scroll.clone())", SIDEBAR)
        self.assertIn("app.apply_rubber() { cx.notify(); }", INTERACTIONS)
        # ReferenceCollection is a nonvirtualized Div; prepaint includes offscreen children.
        self.assertIn("for child in children {\n                            child.prepaint(window, cx);", DIV)

    def test_copy_and_move_have_explicit_operation_intent(self):
        self.assertIn('let place_tip = "Copy All"', SIDEBAR)
        self.assertIn('id("sidebar-drop-zone-copy")', SIDEBAR)
        self.assertIn('Tooltip::new("Move All")', SIDEBAR)
        self.assertIn('id("sidebar-drop-zone-move")', SIDEBAR)
        self.assertIn("transfer_sidebar_drop_zone(this, dest, false)", SIDEBAR)
        self.assertIn("transfer_sidebar_drop_zone(this, dest, true)", SIDEBAR)
        self.assertIn("paths, true, !move_all, move_all", SIDEBAR)
        self.assertIn("policy: ConflictPolicy::Ask", DRAG)
        self.assertIn("drop_is_structurally_blocked(&command, &paths, ctrl, shift)", DRAG)

    def test_only_successful_sources_are_consumed(self):
        self.assertIn("staged.retain(|item| !completed.contains(&item.path))", DRAG)
        bg = source(UI + "app/bg.rs")
        self.assertIn("report.completed.iter().map(|(src, _)| src.clone())", bg)
        self.assertIn("self.consume_completed_staged_paths(&sources)", bg)
        submit = SIDEBAR.split("fn transfer_sidebar_drop_zone(", 1)[1].split("/// A 16px", 1)[0]
        self.assertNotIn(".clear()", submit)
        self.assertIn("cancelled operations must preserve staged sources", DRAG)

    def test_speed_is_independent_and_precision_input_bypasses_it(self):
        smooth = APP.split("fn scroll_smooth_params(", 1)[1].split("impl App", 1)[0]
        self.assertIn("let boost = 1.0;", smooth)
        self.assertNotIn("scroll_speed", smooth.split("let lambda", 1)[1])
        self.assertIn("set_wheel_speed(self.file_scroll_speed())", APP)
        self.assertIn("if matches!(event.delta, crate::ScrollDelta::Lines(_))", DIV)
        self.assertIn("if matches!(delta, crate::ScrollDelta::Lines(_))", DIV)
        self.assertIn("state.wheel_speed.unwrap_or(1.0)", DIV)
        self.assertIn("gpui::ScrollDelta::PreciseLines(l) => l.y * f32::from(row_h)", INTERACTIONS)
        self.assertIn("scroll_speed: 1.0", source("crates/atlas-model/src/settings.rs"))

    def test_cached_input_must_still_have_real_focus(self):
        ext = source("vendor/gpui-component-fork/crates/ui/src/window_ext.rs")
        check = ext.split("fn has_focused_input(&mut self, cx: &mut App) -> bool {", 1)[1].split("#[inline]", 1)[0]
        self.assertIn("focus_handle(cx).is_focused(self)", check)
        self.assertNotIn("focused_input.is_some()", check)
        self.assertIn("self.close_settings_state();", ACTIONS)
        inputs = source(UI + "ui/interactions.rs")
        self.assertIn("window.has_active_dialog(cx)", inputs)
        self.assertIn("window.has_active_sheet(cx)", inputs)


if __name__ == "__main__":
    unittest.main(verbosity=2)
