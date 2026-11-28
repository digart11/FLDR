"""Group 2 source wiring checks. These do not claim runtime/UI validation."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
def src(path):
    return (ROOT / path).read_text(encoding="utf-8")
UI = "crates/atlas-ui/src/"
class Group2Wiring(unittest.TestCase):
    def test_sidebar_has_three_region_ownership_and_small_window_fallback(self):
        text = src(UI + "ui/sidebar.rs")
        for marker in ["sidebar-fixed-navigation", "sidebar-middle-scroll", "sidebar-three-regions", "layout.minimum_height * zoom", "sidebar-drop-zone-expand", "height: shelf_height"]:
            self.assertIn(marker, text)
        self.assertIn("SectionKind::Places | SectionKind::Drives", text)
        self.assertIn("middle = middle.child(section_view)", text)
        self.assertIn('let mut middle = v_flex().w_full().h_auto().flex_none()', text)
        self.assertIn('when(layout.overflow, |outer| outer.overflow_y_scroll()', text)
        self.assertNotIn('sidebar-recent-scroll', text)
        self.assertIn("drive_display_label(&drive.mount_point, &drive.label)", text)
        self.assertIn("ink.progress(frac, bar_color)", text)
    def test_drop_zone_stays_a_fixed_sibling_and_uses_a_neutral_border(self):
        text = src(UI + "ui/sidebar.rs")
        self.assertIn('.child(middle).vertical_scrollbar(&self.scroll))', text)
        self.assertIn('.child(sidebar_drop_zone(', text)
        self.assertIn('drop_border.saturation = 0.0', text)
        self.assertIn('.border_1().border_color(drop_border.opacity(', text)
        self.assertIn('height: shelf_height', text)
        self.assertNotIn('border_color(accent.opacity(if drag_over', text)
    def test_tabs_keep_width_and_have_explicit_overflow(self):
        text = src(UI + "ui/views/tabs_bar_element.rs")
        for marker in ["let tab_min = 160.0 * zoom", "TabHit::PreviousPage", "TabHit::NextPage", "tab_overflow::page_start", "shape_text_truncated", "TabHit::Close", "commit_tab_reorder"]:
            if marker == "commit_tab_reorder":
                self.assertIn(marker, src(UI + "ui/dock/render.rs"))
            else:
                self.assertIn(marker, text)
        self.assertNotIn("Progressive Disclosure", text)
        self.assertIn("last_tab_right = geoms.iter().filter(|geom| geom.visible)", text)
        self.assertIn("tab_overflow::new_tab_x(last_tab_right", text)
        self.assertIn("outline.corner_radii", text)
        self.assertIn("else { inactive_bg }", text)
    def test_commands_use_existing_actions_and_check_selection_destination_and_clipboard(self):
        text = src(UI + "ui/chrome.rs")
        for action in ["NavigateBack", "NavigateForward", "NavigateUp", "Copy", "Cut", "Paste", "Delete", "CreateFolderFocused", "CreateTextFileFocused", "OpenSearchInPlace"]:
            self.assertIn("Action::" + action, text)
        for marker in ["selected_paths_for_pane(pane_id)", "writable && has_clipboard", ".disabled(!enabled)", "this.activate_tab(pane_id)", "file-command-overflow"]:
            self.assertIn(marker, text)
        self.assertIn("IsClipboardFormatAvailable", src(UI + "dnd/windows/ole_clipboard.rs"))
    def test_removed_section_four_does_not_replace_existing_pane_status(self):
        self.assertIn('format!("filter-status-{}", pane_id.0)', src(UI + "ui/chrome.rs"))
        self.assertIn('let status_text = format!("{}/{}", visible.len(), col.entries.len())', src(UI + "ui/views/miller_dg.rs"))
    def test_geometry_uses_existing_storage_atomic_writes_and_background_io(self):
        startup = src(UI + "app/startup.rs")
        for marker in ["observe_window_bounds", "from_millis(750)", "background_executor().spawn", "save_settings_if_current"]:
            self.assertIn(marker, startup)
        self.assertIn("ui::window_geometry::restore(settings.window_geometry.as_ref(), cx)", src(UI + "lib.rs"))
        text = src(UI + "ui/window_geometry.rs")
        for marker in ["display.uuid()", "cx.primary_display()", "display.visible_bounds()", "fit_to_monitor", "is_window_minimized"]:
            self.assertIn(marker, text)
        self.assertIn('write_json("settings.json", settings)', src("crates/atlas-core/src/util/config.rs"))
    def test_native_drag_uses_shell_bitmap_and_preserves_ole_payload_guard(self):
        image = src(UI + "dnd/windows/drag_image.rs")
        for marker in ["InitializeFromBitmap", "SIID_FOLDER", "SIID_DOCNOASSOC", "if count > 1", "count.to_string()", "GetDpiForWindow", "DeleteObject"]:
            self.assertIn(marker, image)
        self.assertNotIn("premultiplied BGRA", image)
        self.assertIn("drag_image::begin", src(UI + "dnd/windows/mod.rs"))
        loop = src(UI + "dnd/windows/mod.rs")
        self.assertLess(loop.index("DoDragDrop(&data_obj"), loop.index("drop(image_session)"))
        data = src(UI + "dnd/windows/data_object.rs")
        self.assertIn("CopyStgMedium(medium)", data)
        self.assertIn("CopyStgMedium(&entry.medium)", data)
        self.assertIn("dynamic_stream_survives_caller_release_and_replacement", data)
        self.assertIn("target.DragOver", image)
        self.assertIn("target.DragLeave", image)
        self.assertIn("target.Drop", image)
        self.assertIn("DRAGDROP_S_USEDEFAULTCURSORS", src(UI + "dnd/windows/drop_source.rs"))
        self.assertIn("AnyDrag::new(DragNoop", src(UI + "ui/drag.rs"))
        self.assertIn("AtlasIcon::Folder", src(UI + "ui/root.rs"))
    def test_text_policy_retains_backend_transparency_guard_and_user_font(self):
        renderer = src("vendor/gpui-ce/crates/gpui/src/window.rs")
        self.assertIn("fn should_use_subpixel_rendering", renderer)
        self.assertIn(".is_transparent()", renderer)
        for view in ["list_dg.rs", "grid_dg.rs"]:
            self.assertIn("name_font.weight = gpui::FontWeight::SEMIBOLD", src(UI + "ui/views/" + view))
        self.assertIn('"Segoe UI"', src(UI + "ui/settings_palette/mod.rs"))
        self.assertIn('.font_family(self.state.settings.font_family.clone())', src(UI + "ui/root.rs"))
        self.assertIn('.recommended_rendering_mode(font_id, font_size)', renderer)
    def test_alternating_rows_are_opt_in_and_list_only_with_state_priority(self):
        model = src("crates/atlas-model/src/settings.rs")
        self.assertIn("alternating_row_colors: false", model)
        text = src(UI + "ui/views/list_dg.rs")
        self.assertIn("if !settings.alternating_row_colors { return None; }", text)
        self.assertIn("alternate_lightness(color.lightness, settings.alternating_row_strength)", text)
        self.assertIn("let mut color = cx.theme().colors.list;", text)
        self.assertLess(text.index("if cell.selected && !cell.editing"), text.index("} else if (cell.pos % self.rows) % 2 == 1"))
        for name in ["grid_dg.rs", "details_element.rs", "tree_dg.rs", "miller_dg.rs"]:
            self.assertNotIn("alternating_row", src(UI + "ui/views/" + name))
    def test_optional_column_separators_use_real_columns_only_in_details(self):
        settings = src("crates/atlas-model/src/settings.rs")
        self.assertIn("detail_column_separators: false", settings)
        self.assertIn("detail_separator_strength: 0.15", settings)
        paint = src(UI + "ui/views/detail_separators.rs")
        for marker in ["if !settings.detail_column_separators", "layout.columns", "column.x", "cx.theme().border", "window.scale_factor()"]:
            self.assertIn(marker, paint)
        body = src(UI + "ui/views/details_element.rs")
        self.assertIn("detail_separators::paint(&app, &prepaint.layout", body)
        self.assertLess(body.index("detail_separators::paint"), body.index("let bg = if sel"))
        self.assertIn("detail_separators::paint(&app, &layout_snap", src(UI + "ui/views/details_header_element.rs"))
        for view in ["list_dg.rs", "grid_dg.rs", "tree_dg.rs", "miller_dg.rs"]:
            self.assertNotIn("detail_separators::paint", src(UI + "ui/views/" + view))
        controls = src(UI + "ui/settings_palette/mod.rs")
        self.assertIn('id: "detail-column-separators"', controls)
        self.assertIn('id: "detail-separator-strength"', controls)
    def test_icon_spacing_changes_label_and_retains_existing_range(self):
        for locale in ["en.yml", "en-US.yml"]:
            self.assertIn('settings_grid_spacing: "Icon Spacing"', src("crates/atlas-ui/locales/" + locale))
        setting = src(UI + "ui/settings_palette/mod.rs").split('id: "grid-spacing",', 1)[1].split("SettingDef {", 1)[0]
        for marker in ["min: 4.0", "max: 48.0", "step: 2.0", "settings.grid_spacing"]:
            self.assertIn(marker, setting)
    def test_theme_editor_keeps_fork_save_discard_and_existing_color_schema(self):
        text = src(UI + "ui/theme_editor.rs")
        self.assertIn("Role::List => cx.theme().colors.list,", text)
        for marker in ["fork_config", "editor.saved = editor.config.clone()", "register_custom_theme(editor.saved)", "save_custom_theme", "config.colors.list_active", "config.colors.accent_foreground", "config.colors.sidebar_foreground", "config.colors.border"]:
            self.assertIn(marker, text)
if __name__ == "__main__":
    unittest.main(verbosity=2)
