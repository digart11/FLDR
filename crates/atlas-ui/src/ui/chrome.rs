//! Window chrome: tab pills, breadcrumb strip, floating search pill, rubber marquee.
#![allow(clippy::too_many_lines)] // reason: cohesive UI block kept whole

use crate::app::App;
use crate::input::action::Action;
use crate::ui::drag_hover::{
    drag_hover_recorder, drag_hover_recorder_with_drop, DragHoverCommand, DragHoverDropCommand,
    DragHoverId, DragHoverKind,
};
use crate::ui::glass::{control_colors, control_hover, glass_shadows_enabled, GlassExt};
use crate::ui::icons::{is_windows_theme, theme_icon, AtlasIcon};
use crate::ui::motion::MotionFeedbackExt as _;
use crate::ui::search_shell::search_icon;
use crate::ui::tooltips::shortcut_tooltip;
use atlas_model::location::Location;
use atlas_model::pane::TabId;
use atlas_model::settings::ViewMode;
use gpui::prelude::*;
use gpui::ColorExt as _;
use gpui::{
    div, px, AnimationExt, AnyElement, Context, Entity, Hsla, MouseButton, SharedString, Window,
};
use gpui_component::button::ButtonVariants as _;
use gpui_component::Selectable as _;
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_component::{h_flex, v_flex, ActiveTheme, ElementExt, Icon, Sizable as _};
use std::collections::HashSet;
use std::path::PathBuf;

/// Shared height of the three floating capsules (filter, branch, controls).
const PILL_HEIGHT: f32 = 30.;

impl App {
    #[allow(dead_code)]
    pub(crate) fn render_floating_search_pill(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let active_theme = self.state.profiles.active().theme.as_str();
        let is_glass = active_theme.ends_with(" Glass");
        let controls = control_colors(cx.theme(), is_glass);
        let windows_theme = is_windows_theme(active_theme);
        let bg = controls.surface;
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(crate::ui::rem_px(18.))
            .flex()
            .justify_center()
            // The wrapper is non-interactive so only the pill captures clicks.
            .child(
                h_flex()
                    .id("search-pill")
                    .h(crate::ui::rem_px(36.))
                    .px_4()
                    .gap_2()
                    .items_center()
                    .rounded_full()
                    .bg(bg)
                    .when(!is_glass, |s| {
                        s.border_1().border_color(cx.theme().border.opacity(0.60))
                    })
                    .when(glass_shadows_enabled(), |s| s.shadow_lg())
                    .cursor_pointer()
                    .text_color(muted)
                    .motion_feedback()
                    .hover(move |s| s.bg(bg.blend(&controls.hover)))
                    .active(move |s| s.bg(bg.blend(&controls.pressed)))
                    .child(if windows_theme {
                        theme_icon(AtlasIcon::Search, 15.0, muted, true)
                    } else {
                        search_icon(muted)
                    })
                    .child(rust_i18n::t!("search"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.process_actions(vec![Action::OpenGotoBar]);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// Phase C: the visible selection rectangle for the active drag, in window coords
    /// (same space as `row_rects`), painted as a deferred layer over the panes. Hidden
    /// until the drag actually moves, so a plain click doesn't flash a box.
    pub(crate) fn render_rubber_marquee(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        // Drawing is now handled natively via canvas in `interactions.rs` (rubber_layer).
        // This avoids DOM layout issues with GPUI's deferred overlays.
        None
    }

    /// Toggle for Digital Grove experimental chrome (inspired by Pexer: O(1) rect-slicing).
    /// Enabled for testing.
    /// Compact breadcrumb strip rendered at the top of each pane, showing the
    /// pane's current path. Clicking any segment navigates that specific pane.
    pub(crate) fn render_crumb_strip(
        &self,
        pane_id: TabId,
        view: &Entity<App>,
        fg: Hsla,
        muted: Hsla,
        border: Hsla,
        _header_bg: Hsla,
        cx: &gpui::App,
    ) -> AnyElement {
        let header_bg = crate::ui::glass::navigation_material(cx.theme(), self.state.profiles.active().theme.ends_with(" Glass"));
        let muted = if self.state.profiles.active().theme.ends_with(" Glass") { crate::ui::glass::secondary_ink(cx.theme()) } else { muted };
        // Each row owns its one material coat, allowing the address field to
        // replace its region rather than layer another tint over navigation.
        let body = self.render_address_editor(pane_id, view, header_bg, cx)
            .unwrap_or_else(|| self.render_crumb_strip_body(pane_id, view, fg, muted, border, header_bg));
        let commands = self.pinned_header_commands(cx);
        let loading = self.state.tab_states.get(&pane_id).is_some_and(|state|
            matches!(state.load_state, atlas_model::tab::LoadState::Loading) ||
            state.miller.columns.iter().any(|column| matches!(column.load_state, atlas_model::tab::LoadState::Loading))) ||
            self.tree_loading.iter().any(|(pane, _)| *pane == pane_id);
        let watcher_failed = self.tab_data_for(pane_id).is_some_and(|tab|
            matches!(tab.location, Location::Directory(_))) && !self.watchers.contains_key(&pane_id);
        let share_list_failed = self.is_network_server_pane(pane_id)
            && self.state.tab_states.get(&pane_id).is_some_and(|state|
                matches!(state.load_state, atlas_model::tab::LoadState::Error(_)));
        let mut tools = h_flex().items_center().gap_1().flex_shrink_0();
        if loading {
            tools = tools.child(crate::ui::loading_spinner::loading_spinner(muted,
                !self.state.settings.disable_animations && !cx.reduce_motion()));
        }
        if watcher_failed || share_list_failed {
            let app = view.clone();
            tools = tools.child(gpui_component::button::Button::new(
                SharedString::from(format!("watcher-refresh-{}", pane_id.0)))
                .ghost().small().icon(Icon::empty().path(AtlasIcon::ArrowClockwise.path()))
                .tooltip(if share_list_failed { "Retry loading network shares".to_string() }
                    else { rust_i18n::t!("watcher_auto_refresh_disabled_help").into_owned() })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    app.update(cx, |this, cx| {
                        this.process_actions(vec![Action::Refresh(pane_id)]);
                        this.notify_all_panes(cx);
                        cx.notify();
                    });
                }));
        }
        // Resolve pane-local state, with no filesystem or clipboard enumeration in render.
        let local_dir = self.tab_data_for(pane_id).and_then(|tab| match &tab.location {
            Location::Directory(path) => Some(path.clone()), _ => None,
        });
        let paths = self.selected_paths_for_pane(pane_id);
        let terminal_dir = local_dir.as_ref().map(|dir| self.context_selection(pane_id).first()
            .filter(|entry| matches!(entry.kind(), atlas_model::file_entry::FileKind::File))
            .and_then(|entry| entry.path().parent().map(std::path::Path::to_path_buf))
            .unwrap_or_else(|| dir.clone()));
        let writable = local_dir.is_some();
        let has_selection = writable && !paths.is_empty();
        #[cfg(windows)]
        let has_clipboard = crate::dnd::windows::ole_clipboard::has_file_format();
        #[cfg(not(windows))]
        let has_clipboard = self.state.ops.clipboard.as_ref().is_some_and(|clipboard| !clipboard.paths.is_empty());
        let actions = vec![
            ("copy", "toolbar_copy", "files.copy", AtlasIcon::Copy, has_selection, Action::Copy(paths.clone()), false),
            ("cut", "toolbar_cut", "files.cut", AtlasIcon::Scissors, has_selection, Action::Cut(paths.clone()), false),
            ("paste", "toolbar_paste", "files.paste", AtlasIcon::Clipboard, writable && has_clipboard, Action::Paste { dest: local_dir.clone().unwrap_or_default() }, false),
            ("rename", "cmd_files_rename_title", "files.rename", AtlasIcon::PencilSimple, has_selection, Action::RenameFocused, false),
            ("delete", "toolbar_delete", "files.trash", AtlasIcon::Trash, has_selection, Action::Delete { paths, to_trash: true }, false),
            ("new-folder", "cmd_files_create_folder_title", "files.create-folder", AtlasIcon::Folder, writable, Action::CreateFolderFocused, false),
            ("new-text-file", "toolbar_new_text", "files.create-text-file", AtlasIcon::File, writable, Action::CreateTextFileFocused, false),
            ("properties", "root_properties", "files.properties", AtlasIcon::Gear, writable, Action::OpenPropertiesFocused, false),
            ("terminal", "cmd_files_open_terminal_title", "files.open-terminal", AtlasIcon::TerminalWindow, terminal_dir.is_some(), Action::OpenTerminal { dir: terminal_dir.clone().unwrap_or_default() }, false),
            ("size", "size_command", "folder.calculate-size", AtlasIcon::FolderMeasure, writable, Action::CalculateFolderSize(pane_id), true),
            ("branch", "toolbar_branch", "view.branch-view", AtlasIcon::GitBranch, writable, Action::ToggleBranchView(pane_id), true),
        ];
        let order = &self.state.settings.toolbar_order;
        let colors = control_colors(cx.theme(), self.state.profiles.active().theme.ends_with(" Glass"));
        let accent = cx.theme().accent;
        let mut hover_surface = colors.surface.blend(&accent.opacity(0.14));
        hover_surface.alpha = 0.90;
        let mut pressed_surface = colors.surface.blend(&accent.opacity(0.24));
        pressed_surface.alpha = 0.96;
        let glow = gpui::BoxShadow { color: accent.opacity(0.12).into(),
            offset: gpui::point(px(0.), px(0.)), blur_radius: px(4.0 * self.state.settings.global_zoom),
            spread_radius: px(0.), inset: false };
        let labels = self.state.settings.toolbar_labels;
        // The existing horizontal overflow owner handles narrow panes and wheel input.
        let visible_count = actions.iter().filter(|(id, ..)| order.iter().any(|item| item == id)).count();
        let min_width = 30. + visible_count as f32 * if labels { 112. } else { 30. };
        tools = tools.w_full().min_w(crate::ui::rem_px(min_width));
        for right in [false, true] {
            if right { tools = tools.child(div().flex_1().min_w(crate::ui::rem_px(8.))); }
            let mut previous_group = None;
            for id in order {
                let Some((_, title, command, glyph, enabled, action, _)) = actions.iter().find(|(key, ..)| key == id) else { continue };
                if actions.iter().find(|(key, ..)| key == id).is_none_or(|item| item.6 != right) { continue; }
                let group = match id.as_str() {
                    "copy" | "cut" | "paste" => 0,
                    "rename" | "delete" => 1,
                    "new-folder" | "new-text-file" => 2,
                    "properties" | "terminal" => 3,
                    _ => 4,
                };
                if previous_group.is_some_and(|previous| previous != group) {
                    tools = tools.child(div().flex_none().w(px(1.)).h(crate::ui::rem_px(16.))
                        .mx(crate::ui::rem_px(3.)).bg(colors.foreground.opacity(0.23)));
                }
                previous_group = Some(group);
                let title = rust_i18n::t!(*title).into_owned();
                let tip = self.command_shortcut_string(command).map_or_else(|| title.clone(), |keys| format!("{title} ({keys})"));
                let active = (*id == "branch" && self.tab_data_for(pane_id).is_some_and(|tab| tab.is_branch_view))
                    || (*id == "split" && self.state.pane_tree.visible_panes().len() > 1);
                let scanning = *id == "size" && self.manual_folder_scan_running();
                let tip = if scanning { rust_i18n::t!("size_cancel_hint").into_owned() }
                    else if id == "terminal" { format!("{tip}\nShift+click: open elevated (UAC)") }
                    else if id == "split" { format!("{tip}\n{}", rust_i18n::t!("toolbar_split_hint")) } else { tip };
                let enabled = *enabled;
                let action = action.clone();
                let terminal = id == "terminal";
                let app = view.clone();
                let glow = glow.clone();
                tools = tools.child(h_flex().id(SharedString::from(format!("file-command-{}-{id}", pane_id.0)))
                    .h(crate::ui::rem_px(26.)).min_w(crate::ui::rem_px(26.)).px(crate::ui::rem_px(5.))
                    .gap_1().items_center().justify_center().flex_none().rounded(crate::ui::rem_px(4.))
                    .border_1().border_color(if active || scanning { colors.border.blend(&cx.theme().accent.opacity(0.35)) } else { gpui::transparent_black() })
                    .when(active || scanning, |button| button.bg(cx.theme().accent.opacity(0.12)))
                    .when(enabled, |button| button.cursor_pointer().motion_feedback()
                        .hover(move |style| style.bg(hover_surface).border_color(accent.opacity(0.38)).shadow(vec![glow.clone()]))
                        .active(move |style| style.bg(pressed_surface).border_color(accent.opacity(0.55))))
                    .child(theme_icon(*glyph, 14., if enabled { colors.foreground } else { muted.opacity(0.45) }, is_windows_theme(&self.state.profiles.active().theme)))
                    .when(labels, |button| button.child(div().text_xs().text_color(if enabled { colors.foreground } else { muted.opacity(0.45) }).child(title)))
                    .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        if !enabled { return; }
                        app.update(cx, |this, cx| {
                            this.activate_tab(pane_id);
                            let chosen = if terminal && window.modifiers().shift {
                                if let Action::OpenTerminal { dir } = &action {
                                    Action::OpenTerminalElevated { dir: dir.clone() }
                                } else { action.clone() }
                            } else { action.clone() };
                            this.process_actions([chosen]);
                            this.notify_all_panes(cx); cx.notify();
                        });
                    }));
            }
        }
        tools = tools.child(self.comparison_button(
            pane_id, view, colors, is_windows_theme(&self.state.profiles.active().theme), cx,
        ));
        for (id, title, icon) in commands {
            let app = view.clone();
            tools = tools.child(gpui_component::button::Button::new(
                SharedString::from(format!("pinned-{}-{id}", pane_id.0)))
                .ghost().small().icon(Icon::empty().path(icon.path()))
                .tooltip(title)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    app.update(cx, |this, cx| {
                        this.run_pinned_header_command(pane_id, &id, cx);
                        this.notify_all_panes(cx);
                        cx.notify();
                    });
                }));
        }
        v_flex().w_full().min_w(px(0.0)).flex_none()
            .child(body)
            .child(CommandStrip { pane: pane_id, tools: tools.into_any_element(), border: cx.theme().border, background: header_bg })
            .into_any_element()
    }

    fn drop_on_breadcrumb(
        view: &Entity<App>,
        pane: TabId,
        paths: &[PathBuf],
        from_self: bool,
        window: &Window,
        cx: &mut gpui::App,
    ) {
        let pos = window.mouse_position();
        let modifiers = window.modifiers();
        let command = view.read(cx).breadcrumb_drop_command_at(pane, pos);
        if let Some(command) = command {
            view.update(cx, |this, cx| {
                this.execute_drag_hover_drop_with_modifiers(
                    command, paths.to_vec(), from_self, modifiers.control, modifiers.shift,
                );
                cx.notify();
            });
            cx.stop_propagation();
        }
    }

    fn render_crumb_strip_body(
        &self,
        pane_id: TabId,
        view: &Entity<App>,
        fg: Hsla,
        muted: Hsla,
        border: Hsla,
        header_bg: Hsla,
    ) -> AnyElement {
        let pane_archive_inner = self.tab_data_for(pane_id).and_then(|t| {
            t.location
                .as_archive()
                .map(|(archive, inner)| (archive.to_path_buf(), inner.to_path_buf()))
        });
        let pane_location = self.tab_data_for(pane_id).map(|t| t.location.clone());
        let (can_back, can_forward, up_location, recent_locations, bookmark_target) = self
            .tab_data_for(pane_id)
            .map(|t| {
                (
                    !t.history.is_empty(),
                    !t.history_forward.is_empty(),
                    t.location.ascend(),
                    self.recent_locations_for_pane(pane_id)
                        .into_iter()
                        .map(|loc| (self.resolved_location_label(&loc), loc))
                        .collect::<Vec<_>>(),
                    self.bookmark_target_for_pane(pane_id),
                )
            })
            .unwrap_or_default();
        let active_theme = self.state.profiles.active().theme.as_str();
        let windows_theme = is_windows_theme(active_theme);
        let is_glass = active_theme.ends_with(" Glass");
        let button_hover = control_hover(fg);

        if let Some(Location::Directory(dir)) = pane_location.as_ref() {
            let trail = self.tab_data_for(pane_id).and_then(|tab| tab.breadcrumb_trail.as_deref());
            let segments = std::sync::Arc::new(crate::ui::chrome_dg::path_with_ghost_segments(dir, trail));
            let is_bookmarked = self.state.profiles.active().bookmarks.iter()
                .any(|bookmark| bookmark.path == *dir);
            let breadcrumb = crate::ui::chrome_dg::DgChromeElement::new(
                pane_id,
                view.downgrade(),
                segments,
                can_back,
                can_forward,
                up_location.is_some(),
                is_bookmarked,
                windows_theme,
                is_glass,
                fg,
                muted,
                header_bg,
                border,
                button_hover,
                fg,
            );
            let internal = view.clone();
            let external = view.clone();
            let hit_view = view.clone();
            return div()
                .id(("directory-breadcrumbs", pane_id.0))
                .w_full()
                .min_w(px(0.))
                .child(breadcrumb)
                .can_drop(move |_, window, cx| {
                    hit_view.read(cx).breadcrumb_drop_command_at(pane_id, window.mouse_position()).is_some()
                })
                .on_drop::<crate::ui::root::DraggedFiles>(move |drag, window, cx| {
                    Self::drop_on_breadcrumb(&internal, pane_id, &drag.paths, true, window, cx);
                })
                .on_drop::<gpui::ExternalPaths>(move |drag, window, cx| {
                    Self::drop_on_breadcrumb(&external, pane_id, drag.paths(), false, window, cx);
                })
                .into_any_element();
        }

        let crumbs: Option<AnyElement> = if let Some((archive, inner)) = pane_archive_inner.as_ref()
        {
            Some(
                archive_breadcrumb(
                    view,
                    pane_id,
                    archive,
                    inner,
                    fg,
                    muted,
                    button_hover,
                    windows_theme,
                )
                .into_any_element(),
            )
        } else {
            pane_location.as_ref().map(|location| match location {
                #[cfg(feature = "provider-host")]
                Location::Remote { id, path } => {
                    let remote_label = self
                        .state
                        .cloud
                        .remote(id)
                        .map(|r| r.name.clone())
                        .unwrap_or_else(|| id.clone());
                    remote_breadcrumb(
                        view,
                        pane_id,
                        id,
                        &remote_label,
                        path,
                        fg,
                        muted,
                        button_hover,
                        windows_theme,
                    )
                    .into_any_element()
                }
                // Virtual locations (tag results, recycle bin) have no
                // filesystem ancestors to climb — show a static labelled crumb
                // instead of a clickable path that would navigate nowhere.
                other => h_flex()
                    .min_w(px(0.))
                    .gap(crate::ui::rem_px(6.))
                    .items_center()
                    .text_color(fg)
                    .child(crate::ui::icons::icon(location_icon(other), 13.0, muted))
                    .child(div().truncate().child(self.resolved_location_label(other)))
                    .into_any_element(),
            })
        };
        let finder_view = view.clone();
        h_flex()
            .w_full()
            .h(crate::ui::rem_px(if windows_theme { 44.0 } else { 36.0 }))
            .px_3()
            .min_w(px(0.))
            .gap(crate::ui::rem_px(if windows_theme { 9.0 } else { 6.0 }))
            .items_center()
            .bg(header_bg)
            .when(!windows_theme, |s| s.text_sm())
            .when(windows_theme, |s| s.text_base())
            .text_color(muted)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                finder_view.update(cx, |this, cx| {
                    this.activate_tab(pane_id);
                    this.open_location_finder_or_editor();
                    cx.notify();
                });
            })
            .child(nav_button(NavButtonProps {
                id: "pane-back",
                pane_id,
                glyph: AtlasIcon::ArrowLeft,
                tooltip_title: rust_i18n::t!("tooltip_nav_back_title").to_string(),
                tooltip_desc: rust_i18n::t!("tooltip_nav_back_desc").to_string(),
                shortcuts: &["alt-left"],
                enabled: can_back,
                action: Action::NavigateBack(pane_id),
                view: view.clone(),
                recent: recent_locations.clone(),
                icon_color: fg,
                hover_bg: button_hover,
                windows_theme,
            }))
            .child(nav_button(NavButtonProps {
                id: "pane-forward",
                pane_id,
                glyph: AtlasIcon::ArrowRight,
                tooltip_title: rust_i18n::t!("tooltip_nav_forward_title").to_string(),
                tooltip_desc: rust_i18n::t!("tooltip_nav_forward_desc").to_string(),
                shortcuts: &["alt-right"],
                enabled: can_forward,
                action: Action::NavigateForward(pane_id),
                view: view.clone(),
                recent: recent_locations.clone(),
                icon_color: fg,
                hover_bg: button_hover,
                windows_theme,
            }))
            .child(nav_button(NavButtonProps {
                id: "pane-up",
                pane_id,
                glyph: AtlasIcon::ArrowUp,
                tooltip_title: rust_i18n::t!("tooltip_nav_up_title").to_string(),
                tooltip_desc: rust_i18n::t!("tooltip_nav_up_desc").to_string(),
                shortcuts: &["alt-up"],
                enabled: up_location.is_some(),
                action: Action::NavigateUp(pane_id),
                view: view.clone(),
                recent: recent_locations.clone(),
                icon_color: fg,
                hover_bg: button_hover,
                windows_theme,
            }))
            .child(bookmark_button(
                pane_id,
                view.clone(),
                bookmark_target.clone(),
                fg,
                button_hover,
                windows_theme,
            ))
            .child(
                div()
                    .flex_none()
                    .w(px(1.))
                    .h(crate::ui::rem_px(if windows_theme { 22.0 } else { 18.0 }))
                    .bg(border.opacity(0.35)),
            )
            .children(crumbs)
            .into_any_element()
    }

    /// User-facing label for a location: tag names resolved from the tag cache
    /// and the recycle bin localized (never the `tag:N` sentinel path).
    pub(crate) fn resolved_location_label(&self, location: &Location) -> SharedString {
        match location {
            Location::Tag(id) => self
                .state
                .tag_cache
                .def(*id)
                .map_or_else(|| location_label(location), |def| def.name.clone().into()),
            Location::RecycleBin => rust_i18n::t!("sidebar_recycle_bin").into_owned().into(),
            _ => location_label(location),
        }
    }

    fn recent_locations_for_pane(&self, pane_id: TabId) -> Vec<Location> {
        let Some(tab) = self.tab_data_for(pane_id) else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        let mut locations = Vec::new();
        seen.insert(tab.location.clone());
        for loc in tab
            .history
            .iter()
            .rev()
            .chain(tab.history_forward.iter().rev())
        {
            if seen.insert(loc.clone()) {
                locations.push(loc.clone());
                if locations.len() == 6 {
                    return locations;
                }
            }
        }
        for path in self.recent_folders().iter().rev() {
            let loc = Location::Directory(path.clone());
            if seen.insert(loc.clone()) {
                locations.push(loc);
                if locations.len() == 6 {
                    break;
                }
            }
        }
        locations
    }

    fn bookmark_target_for_pane(&self, pane_id: TabId) -> Option<BookmarkTarget> {
        let path = self
            .tab_data_for(pane_id)
            .and_then(|t| t.location.fs_dir().map(|p| p.to_path_buf()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| path.display().to_string());
        let index = self
            .state
            .profiles
            .active()
            .bookmarks
            .iter()
            .position(|bookmark| bookmark.path == path);
        Some(BookmarkTarget {
            name,
            path,
            exists: index.is_some(),
            index,
        })
    }

    /// Floating per-pane overlays, anchored to the bottom corners of the file
    /// list: a compact fuzzy-filter pill on the left (a lone magnifier until it
    /// activates) and a controls pill on the right (visibility toggles,
    /// folders-first, view selector). Both sit on translucent glass so the
    /// listing shows through.
    pub(crate) fn render_pane_pills(
        &mut self,
        pane_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, AnyElement) {
        // Mirror the per-tab toggles into the TabState so its cached filtering
        // (and the counts below) pick them up.
        let (folder_order, show_files, show_folders, is_branch_view) = self
            .tab_data_for(pane_id)
            .map(|td| (td.folder_order, td.show_files, td.show_folders, td.is_branch_view))
            .unwrap_or((atlas_model::pane::FolderOrder::default(), true, true, false));
        if let Some(ts) = self.state.tab_states.get_mut(&pane_id) {
            ts.set_visibility(show_files, show_folders);
            ts.is_branch_view = is_branch_view;
            ts.refresh_miller_visibility();
        }
        let flat_counts = self.flat_visible_counts(pane_id);

        let initial = self
            .state
            .tab_states
            .get(&pane_id)
            .map(|ts| ts.search.query.clone())
            .unwrap_or_default();
        let input = self
            .search_inputs
            .entry(pane_id)
            .or_insert_with(|| {
                crate::ui::text_input::single_line(
                    window,
                    cx,
                    rust_i18n::t!("filter_placeholder").into_owned(),
                    initial,
                )
            })
            .clone();

        let value = input.read(cx).value().to_string();
        let mut needs_refresh = false;
        if let Some(ts) = self.state.tab_states.get_mut(&pane_id) {
            // Focus is owned by `sync_overlay_focus`: while `search.active` the
            // field keeps the keyboard, so no per-render focus juggling here.
            ts.search.focus = false;
            if value != ts.search.query {
                ts.search.query.clone_from(&value);
                // Typing directly into the always-visible input activates the filter.
                if !value.is_empty() {
                    ts.search.active = true;
                }
                needs_refresh = true;
            }
        }
        if needs_refresh {
            self.refresh_search_in_place(pane_id);
        }

        let (active, _recursive, status, count, error) = self
            .state
            .tab_states
            .get_mut(&pane_id)
            .map(|ts| {
                (
                    ts.search.active,
                    ts.search.recursive,
                    ts.search.status,
                    ts.visible_indices().len(),
                    ts.search.error.clone(),
                )
            })
            .unwrap_or_default();
        let filtering = active && !value.trim().is_empty();
        let is_glass = self.state.profiles.active().theme.ends_with(" Glass");
        let controls = control_colors(
            cx.theme(),
            is_glass,
        );
        let mut pill_surface = controls.surface;
        // Specify effective alpha directly; do not multiply a translucent surface again.
        pill_surface.alpha = if is_glass { 0.90 } else { 1.0 };
        let (border, muted, _fg, accent, accent_fg) = {
            let t = cx.theme();
            (
                t.border,
                t.muted_foreground,
                t.foreground,
                t.accent,
                t.accent_foreground,
            )
        };
        let view = cx.entity();
        // Concise status inside the pill: result count, "…" while searching,
        // "!" (with the error in its own right) on failure.
        let status_text = if error.is_some() {
            Some("!".to_string())
        } else if status == atlas_model::tab::SearchStatus::Searching {
            Some("…".to_string())
        } else if filtering {
            Some(format!("{count}"))
        } else {
            None
        };
        let windows_theme = is_windows_theme(&self.state.profiles.active().theme);

        // Essential information belongs to every visible pane, independently of focus.
        let focused = self.state.focused_tile == pane_id;

        // ── Left pill: fuzzy filter (lone magnifier until active) ──
        let selected = self.context_selection(pane_id);
        self.sync_context_metadata(pane_id, &selected);
        let zoom = f32::from(window.rem_size()) / crate::ui::BASE_PX;
        let pane_width = self.pane_layout_bounds.get(&pane_id).map_or(640.0, |bounds| f32::from(bounds.size.width) / zoom);
        let calculated_size_text = self.folder_size_text(pane_id);
        let pane_mode = self.tab_data_for(pane_id).map(|tab| tab.view_mode.clone()).unwrap_or_default();
        let (counts, _listing_elapsed) = self.state.tab_states.get_mut(&pane_id).map(|tab| {
            tab.refresh_miller_visibility();
            let counts = if pane_mode == ViewMode::Miller {
                tab.miller.columns.get(tab.miller.focused).map_or(Default::default(), |column|
                    column.visible_counts(&tab.filter, tab.show_files, tab.show_folders))
            } else { flat_counts };
            (counts, tab.listing_elapsed)
        }).unwrap_or_default();
        let total = atlas_model::folder_selection::total(&selected, &self.context_info.stats, std::time::Instant::now());
        let status_height = PILL_HEIGHT;
        let placement = crate::ui::status_layout::placement(pane_width, focused);
        let compact_status = pane_width < 480.0 && !selected.is_empty();
        let selector_open = self.open_view_selectors.contains(&pane_id);
        let metadata = self.context_media_details(pane_id, &selected);
        let size = atlas_core::util::display::format_size_state(total.bytes, total.complete, total.cached);
        let selection_text = if total.show() {
            format!("{} ({size})", rust_i18n::t!("info_selection", count = selected.len()))
        } else { rust_i18n::t!("info_selection", count = selected.len()).into_owned() };
        let scan_tip = calculated_size_text.clone();
        let inspect = view.clone();
        let native_selection = self.tab_data_for(pane_id).is_some_and(|tab| matches!(tab.location, atlas_model::location::Location::Directory(_)));
        let extension = atlas_model::file_presentation::selected_extension(&selected, native_selection).map(|s| format!(".{s}"));
        // Reserve the bounded extension and metadata padding before allowing the
        // essential text to truncate. Completion cannot alter this width budget.
        let essential_max = (pane_width - placement.right - 8.
            - if extension.is_some() { 108. } else { 0. } - 24.).max(80.);
        let details_tip = self.context_details(pane_id, &selected).map(|(_, tip)| tip).unwrap_or_default();
        let file_tip = Some(format!("{} · {}\n{}\n{}",
            rust_i18n::t!("footer_dirs", shown = counts.folders_shown, total = counts.folders_total),
            rust_i18n::t!("footer_files", shown = counts.files_shown, total = counts.files_total),
            if selected.is_empty() { "" } else { &selection_text }, details_tip));
        let status_pill: AnyElement = h_flex().id(SharedString::from(format!("context-info-{}", pane_id.0))).flex_none().min_w_0()
            .max_w(crate::ui::rem_px(essential_max))
            .px_2().min_h(crate::ui::rem_px(PILL_HEIGHT)).items_center().gap_1()
            .rounded_xl().border_1().border_color(controls.border).bg(pill_surface).overflow_hidden()
            .when(is_glass, |s| s.backdrop_blur(px(12.0))).raised_surface()
            .text_xs().font_weight(gpui::FontWeight::NORMAL).text_color(controls.foreground).whitespace_nowrap()
            .child(h_flex().id(SharedString::from(format!("directory-counts-{}", pane_id.0))).flex_none().items_center().gap_1()
                .when_some(scan_tip.filter(|_| !selector_open), |bar, tip| bar.tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
                .child(theme_icon(AtlasIcon::Folder, 13., controls.foreground, windows_theme))
                .child(counts.folders_total.to_string())
                .child(theme_icon(AtlasIcon::File, 13., controls.foreground, windows_theme))
                .child(counts.files_total.to_string()))
            .when(!selected.is_empty(), |bar| bar
                .child(div().text_color(muted).child("—"))
                .child(div().min_w_0().truncate().text_color(accent).child(selection_text)))
            .when_some(file_tip.filter(|_| !selector_open), |bar, tip| bar.tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
            .into_any_element();
        // Metadata uses intrinsic width with shrink only on overflow; no grow.
        // Absolute pane anchor: never introduce a second full-height flow box.
        // Metadata may change or disappear; the essential row and extension do not.
        let status_pill = h_flex().absolute()
            .bottom(crate::ui::rem_px(placement.bottom)).left_2()
            .right(crate::ui::rem_px(placement.right)).gap_1().items_center().min_w_0()
            .child(status_pill)
            .children(extension.map(|text| div().flex_none().max_w(crate::ui::rem_px(100.)).truncate().px_2().min_h(crate::ui::rem_px(PILL_HEIGHT))
                .flex().items_center().rounded_xl().border_1().border_color(controls.border).bg(pill_surface)
                .when(is_glass, |s| s.backdrop_blur(px(12.0))).raised_surface()
                .text_xs().font_weight(gpui::FontWeight::NORMAL).text_color(controls.foreground).child(text)))
            .children(metadata.filter(|(compact, _)| !compact.trim().is_empty()).map(|(compact, expanded)| div()
                .id(SharedString::from(format!("context-metadata-{}", pane_id.0)))
                .flex_initial().min_w_0().overflow_hidden().whitespace_nowrap().px_2()
                .min_h(crate::ui::rem_px(PILL_HEIGHT)).flex().items_center()
                .rounded_xl().border_1().border_color(controls.border).bg(pill_surface)
                .text_xs().font_weight(gpui::FontWeight::NORMAL).text_color(controls.foreground).cursor_pointer()
                .child(div().flex_initial().min_w_0().truncate().child(compact))
                .when(!selector_open, |el| el.tooltip(move |window,cx| gpui_component::tooltip::Tooltip::new(expanded.clone()).build(window,cx)))
                .on_mouse_down(MouseButton::Left,move |_,_,cx| {
                    cx.stop_propagation();inspect.update(cx,|this,cx|{
                        this.activate_tab(pane_id);
                        if !this.state.settings.show_inspector {this.process_actions([Action::ToggleInspector]);}
                        this.notify_all_panes(cx);cx.notify();
                    });
                })))
            .into_any_element();
        let filter_pill: AnyElement = if active && focused {
            let close_view = view.clone();
            div().absolute().inset_0().child(h_flex()
                .id(SharedString::from(format!("filter-pill-{}", pane_id.0)))
                .absolute()
                .bottom(crate::ui::rem_px(placement.bottom + status_height + 8.))
                .max_w(gpui::relative(0.95))
                .left_2()
                .px_2()
                .h(crate::ui::rem_px(PILL_HEIGHT))
                .gap_1()
                .items_center()
                .rounded_xl()
                .border_1()
                .border_color(if filtering {
                    accent.opacity(0.8)
                } else {
                    controls.border
                })
                .bg(pill_surface)
                .when(is_glass, |s| s.backdrop_blur(px(12.0)))
                .raised_surface()
                .text_xs()
                // Keep pill clicks away from the pane underneath (rubber band,
                // pane-focus click handlers).
                .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                    cx.stop_propagation();
                })
                .capture_key_down(
                    cx.listener(move |this, ev: &gpui::KeyDownEvent, _window, cx| {
                        let handled = match ev.keystroke.key.as_str() {
                            "escape" => {
                                this.activate_tab(pane_id);
                                this.process_actions(vec![Action::CloseSearchInPlace]);
                                true
                            }
                            "enter" => {
                                this.open_cursor();
                                true
                            }
                            "down" => {
                                this.nav_down(false, false);
                                true
                            }
                            "up" => {
                                this.nav_up(false, false);
                                true
                            }
                            _ => false,
                        };
                        if handled {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }),
                )
                .child(theme_icon(
                    AtlasIcon::Search,
                    14.0,
                    if filtering { accent } else { muted },
                    windows_theme,
                ))
                .child({
                    let field = div()
                        .overflow_hidden()
                        .child(div().w(crate::ui::rem_px(180.)).child(crate::ui::text_input::flat(&input).xsmall()));
                    // The capsule grows out of the lone magnifier when it opens.
                    if self.state.settings.disable_animations {
                        field.w(crate::ui::rem_px(180.)).into_any_element()
                    } else {
                        field
                            .with_animation(
                                SharedString::from(format!("filter-open-{}", pane_id.0)),
                                crate::ui::motion::animation(crate::ui::motion::MICRO_MS * 2),
                                |this, t| {
                                    let t = 1.0 - (1.0 - t).powi(3);
                                    this.w(crate::ui::rem_px(180. * t)).opacity(t)
                                },
                            )
                            .into_any_element()
                    }
                })
                .child({
                    let branch_view_action = view.clone();
                    div()
                        .id(SharedString::from(format!("filter-branch-{}", pane_id.0)))
                        .p(crate::ui::rem_px(3.))
                        .rounded_md()
                        .cursor_pointer()
                        .bg(controls.hover.opacity(0.0))
                        .when(is_branch_view, |s| s.bg(accent))
                        .motion_feedback()
                        .when(!is_branch_view, |s| s.hover(move |s| s.bg(controls.hover)))
                        .child(theme_icon(
                            AtlasIcon::GitBranch,
                            14.0,
                            if is_branch_view { accent_fg } else { muted },
                            windows_theme,
                        ))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                rust_i18n::t!("cmd_view_branch_view_title").into_owned(),
                            )
                            .build(window, cx)
                        })
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            branch_view_action.update(cx, |this, cx| {
                                this.process_actions(vec![Action::ToggleBranchView(pane_id)]);
                                cx.notify();
                            });
                        })
                })
                .children(status_text.map(|t| {
                    let tip = error.clone();
                    div()
                        .id(SharedString::from(format!("filter-status-{}", pane_id.0)))
                        .text_color(if tip.is_some() { gpui::red() } else { muted })
                        .whitespace_nowrap()
                        .when_some(tip, |s, tip| {
                            s.tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                            })
                        })
                        .child(t)
                }))
                .child(
                    div()
                        .id(SharedString::from(format!("filter-close-{}", pane_id.0)))
                        .p(crate::ui::rem_px(3.))
                        .rounded_md()
                        .cursor_pointer()
                        .bg(controls.hover.opacity(0.0))
                        .motion_feedback()
                        .hover(move |s| s.bg(controls.hover))
                        .active(move |s| s.bg(controls.pressed))
                        .child(theme_icon(AtlasIcon::X, 12.0, muted, windows_theme))
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            close_view.update(cx, |this, cx| {
                                this.process_actions(vec![Action::CloseSearchInPlace]);
                                cx.notify();
                            });
                        }),
                )
                .into_any_element()).child(status_pill).into_any_element()
        } else {
            status_pill
        };

        if !focused { return (filter_pill, div().into_any_element()); }

        // ── Right pill: visibility toggles + folders-first + view selector ──
        let tbtn = |id: String,
                    glyph: AtlasIcon,
                    on: bool,
                    on_color: Hsla,
                    tip: String,
                    action: Action,
                    view: Entity<App>| {
            div()
                .id(SharedString::from(id))
                .flex().flex_none().items_center().gap_1()
                .p(crate::ui::rem_px(4.))
                .rounded_md()
                .cursor_pointer()
                .bg(controls.hover.opacity(0.0))
                .when(on, |s| s.bg(accent.opacity(0.12)))
                .motion_feedback()
                .hover(move |s| s.bg(controls.hover))
                .active(move |s| s.bg(controls.pressed))
                .child(theme_icon(
                    glyph,
                    14.0,
                    if on { on_color } else { controls.foreground },
                    windows_theme,
                ))
                .when(!selector_open, |button| button.tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                }))
                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                    view.update(cx, |this, cx| {
                        this.process_actions(vec![action.clone()]);
                        this.flush_dirty_panes(cx);
                    });
                })
        };

        let view_slider_value = self.pane_view_slider_value(pane_id).max(3.0);
        let zoom_slider = if let Some(slider) = self.pane_zoom_sliders.get(&pane_id) {
            slider.clone()
        } else {
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(3.0)
                    .max(23.0)
                    .step(1.0)
                    .default_value(view_slider_value)
            });
            let sub = cx.subscribe_in(
                &slider,
                window,
                move |this, _, ev: &SliderEvent, _window, cx| {
                    let released = matches!(ev, SliderEvent::Release(_));
                    let latch = this.view_selector_hover.entry(pane_id).or_default();
                    latch.interacting = !released;
                    latch.touch();
                    if !released {
                        this.open_view_selectors.insert(pane_id);
                    } else if !latch.inside {
                        this.selector_hover(pane_id, false, cx);
                    }
                    let value = match ev {
                        SliderEvent::Change(value) | SliderEvent::Release(value) => value.end(),
                    }
                    .round()
                    .clamp(3.0, 23.0);
                    if this.tab_data_for(pane_id).is_some_and(|tab| tab.view_mode == ViewMode::Grid)
                        && this.set_pane_grid_thumbnail_size(pane_id, crate::ui::view_selector_policy::grid_thumbnail_size(value)) {
                        this.notify_pane(pane_id, cx);
                    }
                    cx.notify();
                },
            );
            self.pane_zoom_sliders.insert(pane_id, slider.clone());
            self.pane_zoom_slider_subs.insert(pane_id, sub);
            slider
        };
        let slider_value = zoom_slider.read(cx).value().end();
        if (slider_value - view_slider_value).abs() > f32::EPSILON {
            zoom_slider.update(cx, |slider, cx| {
                slider.set_value(SliderValue::Single(view_slider_value), window, cx);
            });
        }
        let view_btn = self.render_view_selector(pane_id, zoom_slider, window, cx);

        let focus_view = view.clone();
        let order_click_view = view.clone();
        let animate_order = !self.state.settings.disable_animations;
        let order_on = folder_order != atlas_model::pane::FolderOrder::default();
        let order_id = format!("folder-order-{}", pane_id.0);
        let order_icon = div()
            .child(theme_icon(
                AtlasIcon::SortAscending,
                14.0,
                if order_on { accent } else { controls.foreground },
                windows_theme,
            ));
        // Restarts on every state change (the animation id carries the order), so the
        // glyph slides in from below each time the cycle advances.
        let order_icon = if animate_order {
            order_icon
                .with_animation(
                    SharedString::from(format!("{order_id}-{folder_order:?}")),
                    crate::ui::motion::animation(crate::ui::motion::MICRO_MS),
                    |this, t| this.opacity(0.35 + 0.65 * t).mt(px(5.0 * (1.0 - t))),
                )
                .into_any_element()
        } else {
            order_icon.into_any_element()
        };
        let order_tip = format!("{}
{}", rust_i18n::t!("folder_order_choose"), rust_i18n::t!(folder_order.label_key()));
        let order_button = div()
            .id(SharedString::from(order_id))
            .flex().items_center()
            .p(crate::ui::rem_px(4.))
            .rounded_md()
            .cursor_pointer()
            .overflow_hidden()
            .bg(controls.hover.opacity(0.0))
            .when(order_on, |s| s.bg(accent.opacity(0.12)))
            .motion_feedback()
            .hover(move |s| s.bg(controls.hover))
            .active(move |s| s.bg(controls.pressed))
            .child(order_icon)
            .when(!selector_open, |button| button.tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(order_tip.clone()).build(window, cx)
            }))
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                order_click_view.update(cx, |this, cx| {
                    this.activate_tab(pane_id);
                    this.process_actions([Action::ToggleFoldersFirst(pane_id)]);
                    this.notify_pane(pane_id, cx);
                    cx.notify();
                });
            });
        let split_focus = window.use_keyed_state(SharedString::from(format!("bottom-split-focus-{}", pane_id.0)), cx, |_, cx| cx.focus_handle()).read(cx).clone();
        let split_tip = self.command_shortcut_string("pane.toggle-split").map_or_else(
            || rust_i18n::t!("toolbar_split_hint").into_owned(),
            |keys| format!("{} ({keys})", rust_i18n::t!("toolbar_split_hint")));
        let controls_pill = h_flex()
            .id(SharedString::from(format!("controls-pill-{}", pane_id.0)))
            .max_w(crate::ui::rem_px((pane_width - 16.).max(60.)))
            .absolute()
            .bottom_2()
            .right_2()
            .px_1()
            .min_h(crate::ui::rem_px(PILL_HEIGHT)).py(crate::ui::rem_px(4.))
            .gap_1()
            .items_center()
            .rounded_xl()
            .border_1()
            .border_color(controls.border)
            .bg(pill_surface)
            .when(is_glass, |s| s.backdrop_blur(px(12.0)))
            .raised_surface()
            // The view-selector actions act on the focused pane; make sure that's
            // us — and keep the click away from the pane underneath.
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                cx.stop_propagation();
                focus_view.update(cx, |this, _| {
                    this.activate_tab(pane_id);
                });
            })
            .child(
                h_flex().id(SharedString::from(format!("pill-toggles-{}", pane_id.0)))
                .min_w_0().gap_1().overflow_x_scroll()
                .child(tbtn(
                    format!("bb-folders-{}", pane_id.0),
                    AtlasIcon::Folder,
                    !show_folders,
                    accent,
                    format!("{}\n{}\n{}", rust_i18n::t!("footer_dirs", shown = counts.folders_shown, total = counts.folders_total),
                        rust_i18n::t!(if show_folders { "footer_dirs_visible" } else { "footer_dirs_hidden" }),
                        rust_i18n::t!(if self.state.settings.show_hidden { "footer_hidden_included" } else { "footer_hidden_excluded" })),
                    Action::ToggleShowFolders(pane_id),
                    view.clone(),
                ))
                .child(tbtn(
                    format!("bb-files-{}", pane_id.0),
                    AtlasIcon::File,
                    !show_files,
                    accent,
                    format!("{}\n{}\n{}", rust_i18n::t!("footer_files", shown = counts.files_shown, total = counts.files_total),
                        rust_i18n::t!(if show_files { "footer_files_visible" } else { "footer_files_hidden" }),
                        rust_i18n::t!(if self.state.settings.show_hidden { "footer_hidden_included" } else { "footer_hidden_excluded" })),
                    Action::ToggleShowFiles(pane_id),
                    view.clone(),
                ))
                .when(!compact_status, |pill| pill.child(order_button))
                .child(
                    div()
                        .w(px(1.))
                        .h(crate::ui::rem_px(14.))
                        .bg(border.opacity(0.35)),
                )
            )
            .child(div().flex_none().child(
                div().id(SharedString::from(format!("bottom-split-{}", pane_id.0)))
                    .flex().flex_none().items_center().justify_center()
                    .h_5().px_1().rounded(cx.theme().radius)
                    .track_focus(&split_focus.clone().tab_stop(true))
                    .cursor_pointer()
                    .bg(if self.state.pane_tree.visible_panes().len() > 1 { controls.foreground.opacity(0.16) } else { gpui::transparent_black() })
                    .motion_feedback()
                    .hover(move |style| style.bg(controls.hover))
                    .active(move |style| style.bg(controls.pressed))
                    .when(split_focus.is_focused(window), |style| style.shadow(vec![gpui::BoxShadow {
                        color: cx.theme().ring.into(), offset: gpui::point(px(0.), px(0.)),
                        blur_radius: px(0.), spread_radius: px(1.), inset: false,
                    }]))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(split_tip.clone()).build(window, cx))
                    .child(theme_icon(AtlasIcon::Columns, 14., controls.foreground, windows_theme))
                    .on_click({ let view = view.clone(); move |_, _, cx| {
                        view.update(cx, |app, cx| {
                            app.activate_tab(pane_id);
                            app.process_actions([Action::ToggleSplitView]);
                            app.flush_dirty_panes(cx);
                            cx.notify();
                        });
                    } })
            ))
            .child(div().flex_none().child(view_btn))
            .into_any_element();

        (filter_pill, controls_pill)
    }

    #[allow(dead_code)]
    pub(crate) fn render_pane_bottom_bar(
        &mut self,
        pane_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (filter_pill, controls_pill) = self.render_pane_pills(pane_id, window, cx);
        div()
            .relative()
            .size_full()
            .child(filter_pill)
            .child(controls_pill)
            .into_any_element()
    }
}

#[derive(Clone)]
struct BookmarkTarget {
    name: String,
    path: PathBuf,
    exists: bool,
    index: Option<usize>,
}

struct NavButtonProps {
    id: &'static str,
    pane_id: TabId,
    glyph: AtlasIcon,
    tooltip_title: String,
    tooltip_desc: String,
    shortcuts: &'static [&'static str],
    enabled: bool,
    action: Action,
    view: Entity<App>,
    recent: Vec<(SharedString, Location)>,
    icon_color: Hsla,
    hover_bg: Hsla,
    windows_theme: bool,
}

fn nav_button(props: NavButtonProps) -> impl IntoElement {
    let menu_view = props.view.clone();
    let menu_recent = props.recent.clone();
    let hover_id = DragHoverId::Chrome(props.id, props.pane_id);
    let hover_command = match props.action.clone() {
        Action::NavigateBack(tile) => DragHoverCommand::NavigateBack(tile),
        Action::NavigateForward(tile) => DragHoverCommand::NavigateForward(tile),
        Action::NavigateUp(tile) => DragHoverCommand::NavigateUp(tile),
        other => DragHoverCommand::Process(other),
    };
    div()
        .id(SharedString::from(format!(
            "{}-{}",
            props.id, props.pane_id.0
        )))
        .flex_none()
        .size(crate::ui::rem_px(if props.windows_theme {
            34.0
        } else {
            26.0
        }))
        .rounded(px(if props.windows_theme { 8.0 } else { 7.0 }))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(!props.enabled, |s| s.opacity(0.35))
        .bg(props.hover_bg.opacity(0.0))
        .motion_feedback()
        .when(props.enabled, |s| s.hover(move |s| s.bg(props.hover_bg)))
        .child(theme_icon(
            props.glyph,
            if props.windows_theme { 18.0 } else { 15.0 },
            props.icon_color.opacity(0.88),
            props.windows_theme,
        ))
        .when(props.enabled, |s| {
            s.on_prepaint_child(drag_hover_recorder(
                hover_id,
                DragHoverKind::Folder,
                hover_command,
                &props.view,
            ))
        })
        .tooltip(move |window, cx| {
            shortcut_tooltip(
                props.tooltip_title.clone(),
                props.tooltip_desc.clone(),
                props.shortcuts,
                window,
                cx,
            )
        })
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            cx.stop_propagation();
            if props.enabled {
                props.view.update(cx, |this, cx| {
                    this.process_actions(vec![props.action.clone()]);
                    this.flush_dirty_panes(cx);
                });
            }
        })
        .on_mouse_down(MouseButton::Right, {
            let view = menu_view.clone();
            move |_ev, _window, cx| {
                view.update(cx, |this, cx| {
                    this.suppress_next_shell_ctx_menu = true;
                    this.close_ctx_menu(cx);
                });
            }
        })
        .context_menu(move |menu, _window, _cx| {
            nav_context_menu(menu, props.pane_id, menu_view.clone(), menu_recent.clone())
        })
}

fn bookmark_button(
    pane_id: TabId,
    view: Entity<App>,
    bookmark: Option<BookmarkTarget>,
    icon_color: Hsla,
    hover_bg: Hsla,
    windows_theme: bool,
) -> impl IntoElement {
    let enabled = bookmark.as_ref().is_some_and(|b| !b.exists);
    let active = bookmark.as_ref().is_some_and(|b| b.exists);
    let (tooltip_title, tooltip_desc) = if active {
        (
            rust_i18n::t!("tooltip_bookmark_active_title").to_string(),
            rust_i18n::t!("tooltip_bookmark_active_desc").to_string(),
        )
    } else {
        (
            rust_i18n::t!("tooltip_bookmark_add_title").to_string(),
            rust_i18n::t!("tooltip_bookmark_add_desc").to_string(),
        )
    };

    div()
        .id(SharedString::from(format!("pane-bookmark-{}", pane_id.0)))
        .flex_none()
        .size(crate::ui::rem_px(if windows_theme { 34.0 } else { 26.0 }))
        .rounded(px(if windows_theme { 8.0 } else { 7.0 }))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(!enabled && !active, |s| s.opacity(0.35))
        .bg(hover_bg.opacity(0.0))
        .motion_feedback()
        .hover(move |s| s.bg(hover_bg))
        .child(theme_icon(
            AtlasIcon::BookmarkSimple,
            if windows_theme { 18.0 } else { 15.0 },
            if active {
                gpui::yellow()
            } else {
                icon_color.opacity(0.88)
            },
            windows_theme,
        ))
        .tooltip(move |window, cx| {
            shortcut_tooltip(tooltip_title.clone(), tooltip_desc.clone(), &[], window, cx)
        })
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            cx.stop_propagation();
            let Some(bookmark) = bookmark.clone() else {
                return;
            };
            view.update(cx, |this, cx| {
                if let Some(index) = bookmark.index {
                    this.process_actions(vec![Action::RemoveBookmark(index)]);
                } else {
                    this.process_actions(vec![Action::AddBookmark {
                        name: bookmark.name,
                        path: bookmark.path,
                    }]);
                }
                cx.notify();
            });
        })
}

fn nav_context_menu(
    mut menu: PopupMenu,
    pane_id: TabId,
    view: Entity<App>,
    recent: Vec<(SharedString, Location)>,
) -> PopupMenu {
    if recent.is_empty() {
        menu = menu.label(rust_i18n::t!("no_recent_history"));
    } else {
        menu = menu.item(
            PopupMenuItem::new(rust_i18n::t!("recent_places"))
                .icon(Icon::empty().path(AtlasIcon::ClockCounterClockwise.path()))
                .disabled(true),
        );
        for (label, location) in recent.into_iter().take(6) {
            let icon_name = location_icon(&location);
            let item_view = view.clone();
            menu = menu.item(
                PopupMenuItem::new(label)
                    .icon(Icon::empty().path(icon_name.path()))
                    .on_click(move |_ev, _window, cx| {
                        let location = location.clone();
                        item_view.update(cx, |this, cx| {
                            this.process_actions(vec![Action::NavigateLocation {
                                tab: pane_id,
                                location,
                            }]);
                            cx.notify();
                        });
                    }),
            );
        }
    }
    menu
}

fn location_icon(location: &Location) -> AtlasIcon {
    match location {
        Location::Directory(_) => AtlasIcon::Folder,
        Location::Archive { .. } => AtlasIcon::File,
        Location::Tag(_) => AtlasIcon::Tag,
        Location::RecycleBin => AtlasIcon::Trash,
        Location::NetworkServer(_) => AtlasIcon::GlobeSimple,
        #[cfg(feature = "provider-host")]
        Location::Remote { .. } => AtlasIcon::Cloud,
    }
}

fn location_label(location: &Location) -> SharedString {
    let display = location.display_path();
    match location {
        Location::Directory(path) => path.display().to_string().into(),
        Location::Archive { archive, inner } => {
            if inner.as_os_str().is_empty() {
                archive.display().to_string().into()
            } else {
                format!("{} › {}", archive.display(), inner.display()).into()
            }
        }
        Location::Tag(_) | Location::RecycleBin | Location::NetworkServer(_) => display.display().to_string().into(),
        #[cfg(feature = "provider-host")]
        Location::Remote { .. } => display.display().to_string().into(),
    }
}

struct BreadcrumbSegmentProps {
    view: Entity<App>,
    tile: TabId,
    id: SharedString,
    label: String,
    location: Location,
    drop_dest: Option<PathBuf>,
    fg: Hsla,
    layer_base: Hsla,
    windows_theme: bool,
}

fn breadcrumb_segment(props: BreadcrumbSegmentProps) -> gpui::Stateful<gpui::Div> {
    let down_v = props.view.clone();
    let down_location = props.location.clone();
    let hover_bg = props.layer_base;
    let hover_id = match &props.location {
        Location::Directory(path) => DragHoverId::Breadcrumb(props.tile, path.clone()),
        Location::Archive { archive, inner } => {
            DragHoverId::Breadcrumb(props.tile, archive.join(inner))
        }
        _ => DragHoverId::Breadcrumb(props.tile, props.location.display_path()),
    };
    let command = DragHoverCommand::Process(Action::NavigateLocation {
        tab: props.tile,
        location: props.location.clone(),
    });
    let mut el = div()
        .id(props.id.clone())
        .flex_none()
        .relative()
        .h(crate::ui::rem_px(if props.windows_theme {
            32.0
        } else {
            26.0
        }))
        .max_w(crate::ui::rem_px(if props.windows_theme {
            260.0
        } else {
            220.0
        }))
        .px(crate::ui::rem_px(if props.windows_theme {
            10.0
        } else {
            8.0
        }))
        .rounded(px(if props.windows_theme { 7.0 } else { 6.0 }))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(props.fg)
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .child(SharedString::from(props.label))
        .bg(hover_bg.opacity(0.0))
        .motion_feedback()
        .hover(move |s| s.bg(hover_bg))
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            let location = down_location.clone();
            down_v.update(cx, |this, cx| {
                this.state.focused_tile = props.tile;
                this.process_actions(vec![Action::NavigateLocation {
                    tab: props.tile,
                    location,
                }]);
                cx.notify();
            });
            cx.stop_propagation();
        });

    let drop_command = props.drop_dest.map(DragHoverDropCommand::TransferTo)
        .unwrap_or_else(|| crate::ui::drag_hover::location_drop_command(&props.location));
    {
        let drop_v = props.view.clone();
        let drop_v_external = props.view.clone();
        let drop_target = drop_command.clone();
        let drop_target_external = drop_command.clone();
        el = el
            .on_prepaint_child(drag_hover_recorder_with_drop(
                hover_id,
                DragHoverKind::Folder,
                command,
                drop_command,
                &props.view,
            ))
            .on_drop::<crate::ui::root::DraggedFiles>(move |d, window, cx| {
                let paths = d.paths.clone();
                drop_v.update(cx, |this, cx| {
                    let modifiers = window.modifiers();
                    this.execute_drag_hover_drop_with_modifiers(
                        drop_target.clone(),
                        paths,
                        true,
                        modifiers.control,
                        modifiers.shift,
                    );
                    cx.notify();
                });
                cx.stop_propagation();
            })
            .on_drop::<gpui::ExternalPaths>(move |paths, window, cx| {
                let paths = paths.paths().to_vec();
                drop_v_external.update(cx, |this, cx| {
                    let modifiers = window.modifiers();
                    this.execute_drag_hover_drop_with_modifiers(
                        drop_target_external.clone(),
                        paths,
                        false,
                        modifiers.control,
                        modifiers.shift,
                    );
                    cx.notify();
                });
                cx.stop_propagation();
            });
    }

    el
}

fn archive_breadcrumb(
    view: &Entity<App>,
    tile: TabId,
    archive: &std::path::Path,
    inner: &std::path::Path,
    fg: Hsla,
    muted: Hsla,
    layer_base: Hsla,
    windows_theme: bool,
) -> impl IntoElement {
    let mut row = h_flex()
        .flex_1()
        .min_w(px(0.))
        .gap(crate::ui::rem_px(if windows_theme { 7.0 } else { 5.0 }))
        .items_center()
        .overflow_hidden();
    let mut sep = false;

    // Real filesystem ancestors of the archive file.
    let mut cumulative = std::path::PathBuf::new();
    for (i, comp) in archive
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""))
        .components()
        .enumerate()
    {
        cumulative.push(comp.as_os_str());
        let raw = comp.as_os_str().to_string_lossy().into_owned();
        let label = if raw.is_empty() { "/".to_string() } else { raw };
        if sep {
            row = row.child(div().text_color(muted).child("›"));
        }
        sep = true;
        let target = cumulative.clone();
        row = row.child(breadcrumb_segment(BreadcrumbSegmentProps {
            view: view.clone(),
            tile,
            id: SharedString::from(format!("crumb-r{}-{}", tile.0, i)),
            label,
            location: Location::Directory(target.clone()),
            drop_dest: Some(target),
            fg,
            layer_base,
            windows_theme,
        }));
    }

    if sep {
        row = row.child(div().text_color(muted).child("›"));
    }
    let archive_target = archive.to_path_buf();
    row = row.child(breadcrumb_segment(BreadcrumbSegmentProps {
        view: view.clone(),
        tile,
        id: SharedString::from(format!("crumb-archive-{}", tile.0)),
        label: archive
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| archive.display().to_string()),
        location: Location::Archive {
            archive: archive_target,
            inner: PathBuf::new(),
        },
        drop_dest: None,
        fg,
        layer_base,
        windows_theme,
    }));

    // Inner path components within the archive.
    let mut inner_so_far = std::path::PathBuf::new();
    for (j, comp) in inner.components().enumerate() {
        inner_so_far.push(comp.as_os_str());
        let raw = comp.as_os_str().to_string_lossy().into_owned();
        row = row.child(div().text_color(muted).child("›"));
        let archive_path = archive.to_path_buf();
        let inner_target = inner_so_far.clone();
        row = row.child(breadcrumb_segment(BreadcrumbSegmentProps {
            view: view.clone(),
            tile,
            id: SharedString::from(format!("crumb-a{}-{}", tile.0, j)),
            label: raw,
            location: Location::Archive {
                archive: archive_path.clone(),
                inner: inner_target,
            },
            drop_dest: None,
            fg,
            layer_base,
            windows_theme,
        }));
    }

    row
}

#[cfg(feature = "provider-host")]
#[allow(clippy::too_many_arguments)]
fn remote_breadcrumb(
    view: &Entity<App>,
    tile: TabId,
    id: &str,
    remote_label: &str,
    path: &str,
    fg: Hsla,
    muted: Hsla,
    layer_base: Hsla,
    windows_theme: bool,
) -> impl IntoElement {
    let mut row = h_flex()
        .flex_1()
        .min_w(px(0.))
        .gap(crate::ui::rem_px(if windows_theme { 7.0 } else { 5.0 }))
        .items_center()
        .overflow_hidden();

    row = row.child(crate::ui::icons::icon(AtlasIcon::Cloud, 13.0, muted));

    // Root crumb for the remote itself
    row = row.child(breadcrumb_segment(BreadcrumbSegmentProps {
        view: view.clone(),
        tile,
        id: SharedString::from(format!("crumb-remote-root-{}", tile.0)),
        label: remote_label.to_string(),
        location: Location::Remote {
            id: id.to_string(),
            path: String::new(),
        },
        drop_dest: None,
        fg,
        layer_base,
        windows_theme,
    }));

    let trimmed = path.trim_matches('/');
    if !trimmed.is_empty() {
        let mut path_so_far = String::new();
        for (idx, comp) in trimmed.split('/').enumerate() {
            if !path_so_far.is_empty() {
                path_so_far.push('/');
            }
            path_so_far.push_str(comp);
            row = row.child(div().text_color(muted).child("›"));
            row = row.child(breadcrumb_segment(BreadcrumbSegmentProps {
                view: view.clone(),
                tile,
                id: SharedString::from(format!("crumb-remote-sub-{}-{}", tile.0, idx)),
                label: atlas_model::remote_path::file_name(comp),
                location: Location::Remote {
                    id: id.to_string(),
                    path: path_so_far.clone(),
                },
                drop_dest: None,
                fg,
                layer_base,
                windows_theme,
            }));
        }
    }

    row
}

/// Retained per-pane overflow, including a normal wheel on narrow split panes.
#[derive(gpui::IntoElement)]
struct CommandStrip { pane: TabId, tools: AnyElement, border: Hsla, background: Hsla }
impl gpui::RenderOnce for CommandStrip {
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        use gpui_component::scroll::ScrollableElement;
        let state = window.use_keyed_state(SharedString::from(format!("file-command-scroll-{}", self.pane.0)), cx,
            |_, _| gpui::ScrollHandle::new());
        let scroll = state.read(cx).clone();
        let wheel = scroll.clone();
        div().id(SharedString::from(format!("pane-tools-{}", self.pane.0)))
            .w_full().h(crate::ui::rem_px(30.)).min_w(px(0.)).flex_none().relative()
            .bg(self.background).border_b_1().border_color(self.border.opacity(0.2))
            .child(div().id("file-command-overflow").size_full().overflow_x_scroll()
                .track_scroll(&scroll).child(self.tools)
                .on_scroll_wheel(move |event, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height());
                    if delta.x == px(0.) && delta.y != px(0.) {
                        let mut offset = wheel.offset();
                        offset.x -= delta.y;
                        wheel.set_offset(offset);
                        window.refresh();
                        cx.stop_propagation();
                    }
                }))
            .horizontal_scrollbar(&scroll)
    }
}
