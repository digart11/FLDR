//! Central drag-hover navigation.
//!
//! Renderers register window-space targets every frame; the global motor decides
//! when a held drag should navigate or autoscroll. This keeps Finder-style drag
//! affordances out of individual sidebar/chrome/view widgets.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::app::App;
use crate::input::action::Action;
use crate::ui::edge_autoscroll;
use atlas_model::layout_tree::TileId;
use atlas_model::location::Location;
use atlas_model::tag_cache::TagId;
use gpui::prelude::*;
use gpui::{
    canvas, div, AnyElement, Context, DispatchPhase, Entity, MouseMoveEvent, Window,
};
use gpui_component::ActiveTheme;

pub(crate) const NAV_HOVER_DELAY: Duration = Duration::from_millis(600);
pub(crate) const FOLDER_HOVER_DELAY: Duration = Duration::from_millis(700);
const TAB_HOVER_DELAY: Duration = Duration::from_millis(350);
const REPEAT_NAV_GUARD: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DragHoverId {
    Sidebar(String),
    Chrome(&'static str, TileId),
    Folder(TileId, PathBuf),
    Column(TileId, PathBuf),
    Breadcrumb(TileId, PathBuf),
    BreadcrumbMenu(TileId, PathBuf),
    Inspector(PathBuf),
    Tab(TileId),
}

#[derive(Clone)]
pub(crate) enum DragHoverCommand {
    NavigatePath(PathBuf),
    NavigatePathIn(TileId, PathBuf),
    NavigateLocation(Location),
    NavigateBack(TileId),
    NavigateForward(TileId),
    NavigateUp(TileId),
    OpenTag(TagId),
    Process(Action),
}

#[derive(Clone)]
pub(crate) enum DragHoverDropCommand {
    AddBookmarks,
    InsertBookmarks(usize),
    AssignTag(TagId),
    StageInSidebar,
    TransferTo(PathBuf),
    /// Send the dropped paths to the OS trash (drop on the Recycle Bin).
    TrashPaths,
    RejectArchive,
    RejectNetworkServer,
}

#[cfg(windows)]
pub(crate) struct NativeDropSnapshot {
    targets: Vec<DragHoverTarget>,
    dropzone: Option<gpui::Bounds<gpui::Pixels>>,
    sidebar: Option<gpui::Bounds<gpui::Pixels>>,
    tabs: std::collections::HashMap<TileId, DragHoverDropCommand>,
    panes: Vec<(gpui::Bounds<gpui::Pixels>, DragHoverDropCommand)>,
}

#[cfg(windows)]
impl NativeDropSnapshot {
    pub(crate) fn transfer_at(&self, x: f32, y: f32, paths: &[PathBuf], ctrl: bool, shift: bool) -> Option<PathBuf> {
        let command = self.command_at(x, y)?;
        if drop_is_structurally_blocked(&command, paths, ctrl, shift) { return None; }
        match command { DragHoverDropCommand::TransferTo(dest) => Some(dest), _ => None }
    }
    pub(crate) fn command_at(&self, x: f32, y: f32) -> Option<DragHoverDropCommand> {
        let pos = gpui::point(gpui::px(x), gpui::px(y));
        resolved_command_at(&self.targets, pos, self.dropzone, self.sidebar,
            |id| self.tabs.get(&id).cloned(),
            || self.panes.iter().find(|(bounds, _)| contains(*bounds, pos)).map(|(_, command)| command.clone()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragHoverKind {
    Nav,
    Folder,
}

#[derive(Clone)]
pub(crate) struct DragHoverTarget {
    pub(crate) id: DragHoverId,
    pub(crate) bounds: gpui::Bounds<gpui::Pixels>,
    pub(crate) kind: DragHoverKind,
    pub(crate) command: DragHoverCommand,
    pub(crate) drop_command: Option<DragHoverDropCommand>,
}

struct DragNavigationSnapshot {
    location: Location,
    history: Vec<Location>,
    forward: Vec<Location>,
    trail: Option<PathBuf>,
    view: atlas_model::settings::ViewMode,
    sort_col: atlas_model::file_entry::SortColumn,
    sort_dir: atlas_model::file_entry::SortDir,
    columns: Option<atlas_model::profiles::FolderColumns>,
}

#[derive(Default)]
pub(crate) struct DragHoverState {
    reference_drag: bool,
    cloud_drag: bool,
    active: bool,
    pub(crate) cursor: Option<gpui::Point<gpui::Pixels>>,
    active_id: Option<DragHoverId>,
    entered_at: Option<Instant>,
    last_fire: Option<(DragHoverId, Instant)>,
    fired_id: Option<DragHoverId>,
    last_scroll_frame: Option<Instant>,
    pub(crate) blink_id: Option<DragHoverId>,
    pub(crate) blink_started_at: Option<Instant>,
    pub(crate) blink_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    /// Per-pane (location, history, forward) captured when the drag began, so an
    /// ESC-cancel (or drag-out without an in-Atlas drop) can undo any spring-load
    /// navigation that fired while dragging.
    nav_snapshot: Option<std::collections::HashMap<TileId, DragNavigationSnapshot>>,
    visible_tabs: Vec<TileId>,
    /// Focused tile captured when the drag began, so an abandoned drag restores
    /// the pre-drag tab.
    initial_focused_tile: Option<TileId>,
    /// Set once a real drop lands inside Atlas; suppresses the cancel-revert.
    committed: bool,
    /// Time of the last fired nav, used to drive the confirmation pulse glow.
    pub(crate) confirm_pulse_at: Option<Instant>,
    pub(crate) confirm_pulse_bounds: Option<gpui::Bounds<gpui::Pixels>>,
}

impl DragHoverState {
    pub(crate) fn navigation_is_provisional(&self) -> bool {
        self.active && !self.committed
    }

    fn reset_target(&mut self, id: Option<DragHoverId>, now: Instant) {
        if self.active_id == id {
            return;
        }
        self.active_id = id;
        self.entered_at = now.into();
        self.fired_id = None;
        self.blink_id = None;
        self.blink_started_at = None;
        self.blink_bounds = None;
    }

    pub(crate) fn clear(&mut self) {
        self.reference_drag = false;
        self.cloud_drag = false;
        self.active = false;
        self.cursor = None;
        self.active_id = None;
        self.entered_at = None;
        self.last_fire = None;
        self.fired_id = None;
        self.last_scroll_frame = None;
        self.blink_id = None;
        self.blink_started_at = None;
        self.blink_bounds = None;
        self.nav_snapshot = None;
        self.visible_tabs.clear();
        self.initial_focused_tile = None;
        self.committed = false;
        self.confirm_pulse_at = None;
        self.confirm_pulse_bounds = None;
    }
}

impl App {
    #[cfg(feature = "provider-host")]
    pub(crate) fn begin_cloud_drag(&mut self) {
        self.begin_drag_hover();
        self.drag_hover.cloud_drag = true;
    }
    pub(crate) fn begin_bookmark_drag(&mut self) {
        self.begin_drag_hover();
        self.drag_hover.reference_drag = true;
    }
    pub(crate) fn begin_drag_hover(&mut self) {
        if self.drag_hover.active { return; }
        self.drag_hover.active = true;
        self.drag_hover.committed = false;
        self.drag_hover.initial_focused_tile = Some(self.state.focused_tile);
        self.drag_hover.visible_tabs = self.state.pane_tree.visible_panes();
        // Snapshot every pane's nav state so a cancel can undo spring-load nav.
        use atlas_model::layout_tree::Tile;
        let snap: std::collections::HashMap<_, _> = self
            .state
            .pane_tree
            .tiles
            .iter()
            .filter_map(|(id, tile)| match tile {
                Tile::Pane(p) => Some((
                    *id,
                    DragNavigationSnapshot {
                        location: p.location.clone(),
                        history: p.history.clone(),
                        forward: p.history_forward.clone(),
                        trail: p.breadcrumb_trail.clone(),
                        view: p.view_mode.clone(),
                        sort_col: p.sort_col.clone(),
                        sort_dir: p.sort_dir.clone(),
                        columns: p.details_columns.clone(),
                    },
                )),
                _ => None,
            })
            .collect();
        self.drag_hover.nav_snapshot = Some(snap);
    }

    /// Mark the drag as committed (a real drop landed inside Atlas), so the
    /// cancel-revert is suppressed when the drag ends.
    pub(crate) fn mark_drag_committed(&mut self) {
        if self.drag_hover.committed {
            return;
        }
        self.drag_hover.committed = true;
        // Spring-loading must not reorder sidebar destinations under the cursor.
        // Only the final route becomes recent when an actual drop commits it.
        let focused = self.state.focused_tile;
        let mut visited: Vec<_> = self
            .drag_hover
            .nav_snapshot
            .as_ref()
            .into_iter()
            .flat_map(|snapshot| snapshot.iter())
            .filter_map(|(pane, before)| {
                let current = &self.tab_data_for(*pane)?.location;
                match current {
                    Location::Directory(path) if current != &before.location => Some((*pane, path.clone())),
                    _ => None,
                }
            })
            .collect();
        visited.sort_by_key(|(pane, _)| (*pane == focused, pane.0));
        for (_, path) in visited {
            self.push_recent_folder(&path);
        }
    }

    /// Undo any spring-load navigation that fired during the drag, restoring each
    /// pane's location/history to the pre-drag snapshot. Marks affected panes
    /// dirty so the next flush repaints them. No-op once committed.
    fn revert_drag_nav(&mut self) {
        if self.drag_hover.committed {
            return;
        }
        for tab in self.drag_hover.visible_tabs.drain(..) {
            if !self.tab_closing.contains_key(&tab) && !self.pane_closing.contains_key(&tab) {
                self.state.pane_tree.make_active(|id, _| id == tab);
            }
        }
        if let Some(initial) = self.drag_hover.initial_focused_tile.take() {
            if self.state.focused_tile != initial {
                self.activate_tab(initial);
            }
        }
        let Some(snap) = self.drag_hover.nav_snapshot.take() else {
            return;
        };
        use atlas_model::layout_tree::Tile;
        for (id, before) in snap {
            let Some(Tile::Pane(tab)) = self.state.pane_tree.tiles.get_mut(id) else { continue; };
            if tab.location == before.location && tab.history == before.history
                && tab.history_forward == before.forward
            {
                continue;
            }
            tab.location = before.location;
            tab.volume_serial = None;
            tab.reconcile_volume(&self.mounted_volumes);
            tab.history = before.history;
            tab.history_forward = before.forward;
            tab.breadcrumb_trail = before.trail;
            tab.view_mode = before.view;
            tab.sort_col = before.sort_col;
            tab.sort_dir = before.sort_dir;
            tab.details_columns = before.columns;
            let location = tab.location.clone();
            self.dirty_panes.insert(id);
            match &location {
                Location::Directory(path) => self.ensure_pane_watcher(id, path.clone()),
                _ => { self.watchers.remove(&id); }
            }
            if let Some(state) = self.state.tab_states.get_mut(&id) {
                state.begin_navigation();
            }
            self.spawn_list(id, location);
        }
    }

    pub(crate) fn clear_chrome_drop_targets(&mut self, pane: TileId) {
        self.pending_drag_hover_targets.retain(|target| {
            !matches!(target.id, DragHoverId::Chrome(_, id) | DragHoverId::Breadcrumb(id, _) if id == pane)
        });
    }

    pub(crate) fn register_drag_hover_target(&mut self, mut target: DragHoverTarget) {
        if let DragHoverId::Folder(pane, path) | DragHoverId::Column(pane, path) = &target.id {
            if let Some(tab) = self.tab_data_for(*pane) {
                if matches!(tab.location, Location::NetworkServer(_)) {
                    return;
                }
                let location = folder_location(&tab.location, path);
                target.drop_command = Some(location_drop_command(&location));
                target.command = DragHoverCommand::Process(Action::NavigateLocation { tab: *pane, location });
            }
        }
        if let Some(existing) = self
            .pending_drag_hover_targets
            .iter_mut()
            .find(|existing| existing.id == target.id)
        {
            *existing = target;
        } else {
            self.pending_drag_hover_targets.push(target);
        }
    }

    pub(crate) fn drag_hover_maybe_active(&self) -> bool {
        #[cfg(windows)]
        if self
            .dnd
            .as_ref()
            .and_then(|d| d.zones.lock().or_else(atlas_model::sync::recover).ok().map(|z| z.drag_active))
            .unwrap_or(false)
        {
            return true;
        }
        self.drag_hover.active
    }

    /// Check if ANY drag is currently active across GPUI internal drag,
    /// Windows OLE external drag, drag-hover, or rubber-band selection.
    pub(crate) fn is_drag_active(&self, has_gpui_drag: bool) -> bool {
        has_gpui_drag
            || self.drag_hover_maybe_active()
            || self.active_external_drag.is_some()
            || self.rubber.is_some()
    }

    pub(crate) fn drag_hover_set_cursor(&mut self, pos: gpui::Point<gpui::Pixels>) {
        if !self.drag_hover.active && self.drag_hover_maybe_active() {
            self.begin_drag_hover();
        }
        self.drag_hover.cursor = Some(pos);
    }

    pub(crate) fn drag_hover_external_left(&mut self) {
        // OLE calls IDropTarget::DragLeave on ESC-cancel and when the drag leaves
        // our window. A real drop calls Drop (→ mark_drag_committed) instead, so a
        // DragLeave without a commit means the drag was abandoned → undo nav.
        self.revert_drag_nav();
        self.drag_hover.clear();
        self.active_external_drag = None;
    }

    pub(crate) fn tick_drag_hover(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        if let Some((x, y)) = self
            .dnd
            .as_ref()
            .and_then(|d| d.zones.lock().or_else(atlas_model::sync::recover).ok().and_then(|z| z.cursor))
        {
            self.drag_hover_set_cursor(gpui::point(gpui::px(x), gpui::px(y)));
        }
        // A release already queued by OLE must be handled against the view
        // under the pointer before a dwell can switch tabs or navigate again.
        #[cfg(windows)]
        let mut dirty = self.drain_dnd_queue();
        #[cfg(not(windows))]
        let mut dirty = false;
        if self.drag_hover.active {
            dirty |= self.tick_drag_hover_inner();
        }
        #[cfg(windows)]
        self.publish_native_drop_snapshot();
        // During a Windows OLE drag the modal `DoDragDrop` loop starves gpui's
        // async 60 Hz drain, so spawn_list results and scroll offsets are applied
        // but never repainted. This canvas DOES run each frame (WM_PAINT is still
        // serviced), so pump the drain + flush here to keep the view live: the
        // spring-loaded folder shows its new contents and edge-scroll visibly moves.
        if dirty || !self.dirty_panes.is_empty() {
            self.drain_bg_results_state_only();
            self.flush_dirty_panes(cx);
            cx.notify();
        }
    }

    /// Drives one frame of hover navigation + edge-scroll. Returns `true` when a
    /// navigation fired, a scroll happened, or the blink state changed — i.e. the
    /// caller should repaint.
    fn tick_drag_hover_inner(&mut self) -> bool {
        let Some(pos) = self.drag_hover.cursor else {
            return false;
        };
        let now = Instant::now();
        let in_dropzone = self.dropzone_bounds.is_some_and(|b| contains(b, pos));
        let hit = hover_target_at(
            &self.pending_drag_hover_targets,
            pos,
            self.dropzone_bounds,
            self.sidebar_scroll_region.as_ref().map(|(bounds, _)| *bounds),
        )
        .cloned();

        // A drop over the fixed shelf must not scroll or activate sidebar rows
        // beneath it, even when their retained bounds overlap the shelf.
        let scrolled = !in_dropzone && self.tick_edge_scroll(pos, now);

        let Some(target) = hit else {
            let changed = self.drag_hover.active_id.is_some();
            self.drag_hover.reset_target(None, now);
            return scrolled || changed;
        };
        if matches!(target.id, DragHoverId::Breadcrumb(..))
            && matches!(target.drop_command, Some(DragHoverDropCommand::RejectNetworkServer))
        {
            let changed = self.drag_hover.active_id.is_some();
            self.drag_hover.reset_target(None, now);
            return scrolled || changed;
        }
        if let DragHoverId::Tab(pane_id) = target.id {
            let is_already_active = self
                .state
                .find_tabs_container(pane_id)
                .and_then(|cid| match self.state.pane_tree.tiles.get(cid) {
                    Some(atlas_model::layout_tree::Tile::Container(
                        atlas_model::layout_tree::Container::Tabs(tabs),
                    )) => tabs.active,
                    _ => None,
                })
                .map_or(self.state.focused_tile == pane_id, |active_id| active_id == pane_id);
            if is_already_active {
                self.drag_hover.reset_target(None, now);
                return scrolled;
            }
        }
        let changed = self.drag_hover.active_id.as_ref() != Some(&target.id);
        self.drag_hover.reset_target(Some(target.id.clone()), now);
        let scrolled = scrolled || changed;
        if self.drag_hover.reference_drag {
            return scrolled; // Reordering references never spring-loads a folder.
        }
        let delay = match (&target.id, target.kind) {
            (DragHoverId::Tab(_), _) => TAB_HOVER_DELAY,
            (_, DragHoverKind::Nav) => NAV_HOVER_DELAY,
            (_, DragHoverKind::Folder) => FOLDER_HOVER_DELAY,
        };
        let entered = self.drag_hover.entered_at.unwrap_or(now);
        if now.duration_since(entered) < delay {
            return scrolled;
        }
        if target.kind == DragHoverKind::Folder
            && self.drag_hover.blink_id.as_ref() != Some(&target.id)
        {
            self.drag_hover.blink_id = Some(target.id.clone());
            self.drag_hover.blink_started_at = Some(now);
            self.drag_hover.blink_bounds = Some(target.bounds);
            return true;
        }
        if target.kind == DragHoverKind::Folder
            && self
                .drag_hover
                .blink_started_at
                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(170))
        {
            return scrolled;
        }
        if self
            .drag_hover
            .fired_id
            .as_ref()
            .is_some_and(|id| id == &target.id)
        {
            return scrolled;
        }
        if self
            .drag_hover
            .last_fire
            .as_ref()
            .is_some_and(|(id, at)| id == &target.id && now.duration_since(*at) < REPEAT_NAV_GUARD)
        {
            return scrolled;
        }

        self.execute_drag_hover_command(target.command);
        self.drag_hover.last_fire = Some((target.id.clone(), now));
        self.drag_hover.fired_id = Some(target.id.clone());
        // Hand the dwell glow off to a short confirmation pulse on the same rect.
        self.drag_hover.confirm_pulse_at = Some(now);
        self.drag_hover.confirm_pulse_bounds =
            self.drag_hover.blink_bounds.take().or(Some(target.bounds));
        self.drag_hover.blink_id = None;
        self.drag_hover.blink_started_at = None;
        true
    }

    /// The destination is visible immediately; dwell/navigation feedback is a
    /// separate layer so the user never has to wait for spring-loading to aim.
    pub(crate) fn render_drag_hover_blink(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let destination = self.render_drag_destination(window, cx);
        let navigation = self.render_drag_navigation_blink(cx);
        if destination.is_none() && navigation.is_none() {
            return None;
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .children(destination)
                .children(navigation)
                .into_any_element(),
        )
    }

    fn render_drag_destination(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.drag_hover_maybe_active() {
            return None;
        }
        let pos = self.drag_hover.cursor?;
        if self.drag_hover.reference_drag && self.bookmark_removal_at(pos) {
            return Some(drag_badge(rust_i18n::t!("sidebar_bookmark_remove_drop").into_owned(), pos, window, cx));
        }
        let sidebar_viewport = self
            .sidebar_scroll_region
            .as_ref()
            .map(|(bounds, _)| *bounds);
        let (bounds, command) = if let Some(bounds) = self.dropzone_bounds.filter(|b| contains(*b, pos)) {
            (bounds, DragHoverDropCommand::StageInSidebar)
        } else {
            let target = hover_target_at(
                &self.pending_drag_hover_targets,
                pos,
                self.dropzone_bounds,
                sidebar_viewport,
            );
            if let Some(target) = target {
                let command = resolve_drop_command(target, |pane| self.tab_drop_command(pane))?;
                let bounds = if matches!(target.id, DragHoverId::Sidebar(_)) {
                    sidebar_viewport.map_or(target.bounds, |viewport| target.bounds.intersect(&viewport))
                } else {
                    target.bounds
                };
                (bounds, command)
            } else {
                let command = self.drag_hover_drop_command_at(f32::from(pos.x), f32::from(pos.y))?;
                let bounds = self.pane_view_bounds.values().find(|b| contains(**b, pos)).copied()?;
                (bounds, command)
            }
        };
        if self.drag_hover.reference_drag && !matches!(command, DragHoverDropCommand::InsertBookmarks(_)) {
            return None;
        }
        let mut paths = self.active_external_drag.as_ref().map(|drag| drag.paths.clone()).unwrap_or_default();
        if let Some(drag) = cx.active_drag() {
            if let Some(external) = drag.value.downcast_ref::<gpui::ExternalPaths>() {
                paths = external.paths().to_vec();
            } else if let Some(files) = drag.value.downcast_ref::<crate::ui::root::DraggedFiles>() {
                paths.clone_from(&files.paths);
            }
        }
        let (mut ctrl, mut shift) = (window.modifiers().control, window.modifiers().shift);
        #[cfg(windows)]
        if let Some(zones) = self.dnd.as_ref().and_then(|dnd| dnd.zones.lock().or_else(atlas_model::sync::recover).ok()) {
            if zones.drag_active {
                if !zones.paths.is_empty() { paths.clone_from(&zones.paths); }
                ctrl = zones.ctrl;
                shift = zones.shift;
            }
        }
        let cloud = cx.active_drag().and_then(|drag| drag.value.downcast_ref::<crate::ui::root::DraggedCloudFiles>());
        let intent = if let Some(cloud) = cloud {
            paths = cloud.paths.clone();
            #[cfg(feature = "provider-host")]
            let allowed = self.cloud_drag_destination_allowed(cloud, &command, shift);
            #[cfg(not(feature = "provider-host"))]
            let allowed = false;
            if allowed { DropIntent::Copy } else { DropIntent::Blocked }
        } else { drop_intent(&command, &paths, ctrl, shift) };
        if matches!(command, DragHoverDropCommand::RejectNetworkServer) {
            return None;
        }
        let tag = match &command {
            DragHoverDropCommand::AssignTag(id) => self.state.tag_cache.def(*id),
            _ => None,
        };
        let color = if matches!(intent, DropIntent::Trash | DropIntent::Blocked) {
            cx.theme().danger
        } else if let Some(tag) = tag {
            gpui::rgb_to_hsla(gpui::rgb((u32::from(tag.color[0]) << 16) | (u32::from(tag.color[1]) << 8) | u32::from(tag.color[2])))
        } else { cx.theme().accent };
        let mut fill = color;
        fill.alpha *= 0.10;
        let mut ring = color;
        ring.alpha *= 0.80;
        let mut glow = color;
        glow.alpha = 0.0;
        let key = match intent {
            DropIntent::Copy => "dnd_intent_copy",
            DropIntent::Move => "dnd_intent_move",
            DropIntent::Trash => "dnd_intent_trash",
            DropIntent::Tag => "dnd_intent_tag",
            DropIntent::Bookmark => "dnd_intent_bookmark",
            DropIntent::Stage => "dnd_intent_stage",
            DropIntent::Blocked => "dnd_intent_blocked",
        };
        let label = rust_i18n::t!(key).into_owned();
        let label = tag.map_or_else(|| label.clone(), |tag| format!("{label} «{}»", tag.name));
        let label = if paths.len() > 1 { format!("{label} · {}", paths.len()) } else { label };
        #[cfg(feature = "provider-host")]
        let label = if let DragHoverDropCommand::TransferTo(destination) = &command {
            format!("{label} → {}", self.cloud_drop_label(destination))
        } else { label };
        Some(div().absolute().inset_0()
            .children((bounds.size.height <= gpui::px(MAX_MARKED_TARGET_H)
                && !matches!(command, DragHoverDropCommand::StageInSidebar)).then(|| {
                glow_rect(bounds, fill, ring, glow, 0.0, 1.5)
            }))
            .child(drag_badge(label, pos, window, cx))
            .into_any_element())
    }

    /// Spring-load affordance: a rhythmically pulsing accent glow on the dwell
    /// target, plus a short confirmation pop when the navigation fires. Driven by
    /// elapsed time — the drag canvas re-renders every frame, so no `with_animation`.
    fn render_drag_navigation_blink(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::ui::motion::ease_out_quart;
        let accent = cx.theme().accent;
        let now = Instant::now();

        // Confirmation pulse takes priority: a bright ring that fades out fast.
        if let (Some(at), Some(bounds)) = (
            self.drag_hover.confirm_pulse_at,
            self.drag_hover.confirm_pulse_bounds,
        ) {
            let t = now.duration_since(at).as_secs_f32() / 0.20;
            if t <= 1.0 {
                let e = ease_out_quart(t);
                let mut fill = accent;
                fill.alpha = 0.34 * (1.0 - e);
                let mut ring = accent;
                ring.alpha = 0.85 * (1.0 - e);
                let mut glow = accent;
                glow.alpha = 0.5 * (1.0 - e);
                return Some(glow_rect(bounds, fill, ring, glow, 8.0 + 22.0 * e, 2.0));
            }
        }

        let bounds = self.drag_hover.blink_bounds?;
        let started = self.drag_hover.blink_started_at.unwrap_or(now);
        let elapsed = now.duration_since(started).as_secs_f32();
        let fade_in = ease_out_quart((elapsed / 0.18).clamp(0.0, 1.0));
        // ~1.6 Hz pulse mapped to 0..1.
        let pulse = if cx.reduce_motion() {
            0.5
        } else {
            gpui::Motion::new(gpui::millis(312))
                .with_easing(gpui::ease_in_out)
                .repeat_forever()
                .alternate()
                .sample(now.duration_since(started))
                .progress
                .get()
        };
        let mut fill = accent;
        fill.alpha = (0.12 + 0.16 * pulse) * fade_in;
        let mut ring = accent;
        ring.alpha = (0.40 + 0.38 * pulse) * fade_in;
        let mut glow = accent;
        glow.alpha = (0.30 + 0.30 * pulse) * fade_in;
        Some(glow_rect(
            bounds,
            fill,
            ring,
            glow,
            10.0 + 12.0 * pulse,
            1.5,
        ))
    }

    pub(crate) fn breadcrumb_drop_command_at(
        &self,
        pane: TileId,
        pos: gpui::Point<gpui::Pixels>,
    ) -> Option<DragHoverDropCommand> {
        let target = hover_target_at(
            &self.pending_drag_hover_targets, pos, self.dropzone_bounds,
            self.sidebar_scroll_region.as_ref().map(|(bounds, _)| *bounds),
        )?;
        if matches!(target.id, DragHoverId::Breadcrumb(id, _) if id == pane)
            && self.pane_accepts_visible_drop(pane)
        {
            return target.drop_command.clone();
        }
        None
    }

    pub(crate) fn drop_files_on_tab(
        view: &Entity<App>,
        paths: &[PathBuf],
        from_self: bool,
        window: &Window,
        cx: &mut gpui::App,
    ) {
        let pos = window.mouse_position();
        let modifiers = window.modifiers();
        view.update(cx, |this, cx| {
            let command = this.tab_drop_target_at(f32::from(pos.x), f32::from(pos.y))
                .and_then(|tab| this.tab_drop_command(tab));
            if let Some(command) = command {
                this.execute_drag_hover_drop_with_modifiers(
                    command, paths.to_vec(), from_self, modifiers.control, modifiers.shift,
                );
                cx.notify();
            }
        });
        cx.stop_propagation();
    }

    pub(crate) fn tab_drop_target_at(&self, x: f32, y: f32) -> Option<TileId> {
        let pos = gpui::point(gpui::px(x), gpui::px(y));
        hover_target_at(
            &self.pending_drag_hover_targets,
            pos,
            self.dropzone_bounds,
            self.sidebar_scroll_region.as_ref().map(|(bounds, _)| *bounds),
        ).and_then(|target| match target.id {
            DragHoverId::Tab(tab) => Some(tab), _ => None,
        })
    }

    pub(crate) fn drag_hover_drop_command_at(
        &self,
        x: f32,
        y: f32,
    ) -> Option<DragHoverDropCommand> {
        let pos = gpui::Point {
            x: gpui::px(x),
            y: gpui::px(y),
        };
        resolved_command_at(
            &self.pending_drag_hover_targets,
            pos,
            self.dropzone_bounds,
            self.sidebar_scroll_region.as_ref().map(|(bounds, _)| *bounds),
            |id| self.tab_drop_command(id),
            || {
            self.pane_view_bounds
                .iter()
                .chain(self.pane_layout_bounds.iter())
                .find(|(pane, bounds)| contains(**bounds, pos) && self.pane_accepts_visible_drop(**pane))
                .and_then(|(pane, _)| {
                    self.tab_drop_command(*pane)
                })
        })
    }

    #[cfg(windows)]
    pub(crate) fn publish_native_drop_snapshot(&self) {
        let Some(dnd) = &self.dnd else { return; };
        if !dnd.zones.lock().or_else(atlas_model::sync::recover).ok().is_some_and(|zones| zones.drag_active) { return; }
        let tabs = self.pending_drag_hover_targets.iter().filter_map(|target| {
            let DragHoverId::Tab(id) = target.id else { return None; };
            self.tab_drop_command(id).map(|command| (id, command))
        }).collect();
        let panes = self.pane_view_bounds.iter().chain(self.pane_layout_bounds.iter())
            .filter(|(id, _)| self.pane_accepts_visible_drop(**id))
            .filter_map(|(id, bounds)| self.tab_drop_command(*id).map(|command| (*bounds, command))).collect();
        let snapshot = NativeDropSnapshot { targets: self.pending_drag_hover_targets.clone(),
            dropzone: self.dropzone_bounds, sidebar: self.sidebar_scroll_region.as_ref().map(|(bounds, _)| *bounds), tabs, panes };
        if let Some(mut zones) = dnd.zones.lock().or_else(atlas_model::sync::recover).ok() { zones.hover_snapshot = Some(snapshot); }
    }

    pub(crate) fn tab_drop_command(&self, pane_id: TileId) -> Option<DragHoverDropCommand> {
        if self.tab_closing.contains_key(&pane_id) || self.pane_closing.contains_key(&pane_id) {
            return None;
        }
        self.tab_data_for(pane_id).map(|tab| location_drop_command(&tab.location))
    }

    pub(crate) fn folder_drop_command(&self, pane: TileId, path: &std::path::Path) -> DragHoverDropCommand {
        self.tab_data_for(pane).map_or_else(
            || DragHoverDropCommand::TransferTo(path.to_path_buf()),
            |tab| if matches!(tab.location, Location::NetworkServer(_)) {
                location_drop_command(&tab.location)
            } else {
                location_drop_command(&folder_location(&tab.location, path))
            },
        )
    }

    pub(crate) fn active_drag_hover_drop_command(&self) -> Option<DragHoverDropCommand> {
        let pos = self.drag_hover.cursor?;
        self.drag_hover_drop_command_at(f32::from(pos.x), f32::from(pos.y))
    }

    pub(crate) fn pane_drop_dest_at(&self, x: f32, y: f32) -> Option<PathBuf> {
        let pos = gpui::Point {
            x: gpui::px(x),
            y: gpui::px(y),
        };
        for (pane, bounds) in self
            .pane_view_bounds
            .iter()
            .chain(self.pane_layout_bounds.iter())
        {
            if contains(*bounds, pos) && self.pane_accepts_visible_drop(*pane) {
                if let Some(dest) = self
                    .tab_data_for(*pane)
                    .and_then(|tab| tab.location.fs_dir().map(|path| path.to_path_buf()))
                {
                    return Some(dest);
                }
            }
        }
        None
    }

    pub(crate) fn pane_accepts_visible_drop(&self, pane: TileId) -> bool {
        if self.tab_closing.contains_key(&pane) || self.pane_closing.contains_key(&pane) {
            return false;
        }
        self.state.find_tabs_container(pane).is_some_and(|container| {
            matches!(self.state.pane_tree.tiles.get(container),
                Some(atlas_model::layout_tree::Tile::Container(atlas_model::layout_tree::Container::Tabs(tabs))) if tabs.active == Some(pane))
        })
    }

    pub(crate) fn execute_drag_hover_drop_command(
        &mut self,
        command: DragHoverDropCommand,
        paths: Vec<PathBuf>,
        from_self: bool,
    ) {
        self.execute_drag_hover_drop_with_modifiers(command, paths, from_self, false, false);
    }

    pub(crate) fn execute_drag_hover_drop_with_modifiers(
        &mut self,
        command: DragHoverDropCommand,
        paths: Vec<PathBuf>,
        from_self: bool,
        ctrl: bool,
        shift: bool,
    ) {
        if paths.is_empty() || drop_is_structurally_blocked(&command, &paths, ctrl, shift) {
            self.toast_error(rust_i18n::t!("dnd_intent_blocked").into_owned());
            self.cancel_dropzone_file_drag();
            return;
        }
        self.mark_drag_committed();
        self.rubber = None;
        match command {
            DragHoverDropCommand::AddBookmarks | DragHoverDropCommand::InsertBookmarks(_) => {
                let index = match command {
                    DragHoverDropCommand::InsertBookmarks(index) => index,
                    _ => self.state.profiles.active().bookmarks.len(),
                };
                self.insert_bookmark_paths(paths, index);
            }
            DragHoverDropCommand::AssignTag(tag) => {
                for path in &paths {
                    if self.state.sidebar_drop_zone.iter().any(|item| &item.path == path) {
                        self.dropzone_pending_tags.insert(path.clone(), tag);
                    }
                }
                self.process_actions(vec![Action::AssignTagToFiles { tag, paths }]);
            }
            DragHoverDropCommand::StageInSidebar => {
                stage_paths(&mut self.state.sidebar_drop_zone, paths, from_self);
                self.cancel_dropzone_file_drag();
            }
            DragHoverDropCommand::TransferTo(dest) => {
                let mut request = drop_transfer_with_modifiers(paths, dest, from_self, ctrl, shift);
                let target = drop_path_key(&request.dest);
                let mut seen = std::collections::HashSet::new();
                let moving = matches!(request.kind, atlas_model::ops::TransferKind::Move);
                request.sources.retain(|path| {
                    let source = drop_path_key(path);
                    valid_drop_source(path, &source, &target, moving) && seen.insert(source)
                });
                if !request.sources.is_empty() {
                    self.process_actions(vec![Action::Transfer(request)]);
                }
            }
            DragHoverDropCommand::TrashPaths => {
                // Direct call (not Action::Delete): that action re-routes by the
                // *focused* tab and would purge instead of trash when a recycle
                // pane happens to hold focus.
                self.delete_files(paths, true);
            }
            DragHoverDropCommand::RejectArchive => {
                self.toast_error(rust_i18n::t!("dnd_archive_drop_blocked").into_owned());
            }
            DragHoverDropCommand::RejectNetworkServer => {}
        }
        // Staged references are consumed by completion, never by submission.
        self.cancel_dropzone_file_drag();
    }

    pub(crate) fn drop_files_on_pane(
        &mut self,
        pane_id: TileId,
        paths: Vec<PathBuf>,
        external: bool,
    ) {
        if let Some(command) = self.tab_drop_command(pane_id) {
            self.execute_drag_hover_drop_command(command, paths, !external);
        }
    }

    pub(crate) fn begin_dropzone_file_drag(&mut self, paths: &[PathBuf]) {
        self.dropzone_drag_paths = paths.iter().cloned().collect();
    }

    pub(crate) fn cancel_dropzone_file_drag(&mut self) {
        self.dropzone_drag_paths.clear();
    }

    pub(crate) fn consume_completed_staged_paths(&mut self, paths: &[PathBuf]) {
        let committed = paths.iter().cloned().collect::<std::collections::HashSet<_>>();
        if committed.is_empty() {
            return;
        }
        consume_staged_paths(&mut self.state.sidebar_drop_zone, &committed);
        self.dropzone_selection.selected
            .retain(|path| !committed.contains(path));
        self.dropzone_rects
            .retain(|(path, _)| !committed.contains(path));
        self.dropzone_drag_paths.retain(|path| !committed.contains(path));
        self.dropzone_pending_tags.retain(|path, _| !committed.contains(path));
    }

    /// DnD uses proximity-only edge activation. The shared core integrates the
    /// velocity by elapsed time so a stationary cursor keeps scrolling smoothly.
    fn tick_edge_scroll(&mut self, pos: gpui::Point<gpui::Pixels>, now: Instant) -> bool {
        let elapsed = self
            .drag_hover
            .last_scroll_frame
            .replace(now)
            .map_or(1.0 / 60.0, |last| now.duration_since(last).as_secs_f32());
        // Panes first (row-based). The sidebar (pixel-based) is the fallback so a
        // pane that overlaps the sidebar bounds still wins.
        if let Some((&pane, bounds)) = self
            .pane_view_bounds
            .iter()
            .find(|(_, bounds)| contains(**bounds, pos))
        {
            return self.tick_pane_edge_scroll(pane, *bounds, pos, elapsed);
        }
        if let Some(handle) = self.dropzone_scroll.clone().filter(|_| self.dropzone_bounds.is_some_and(|bounds| contains(bounds, pos))) {
            let bounds = handle.bounds();
            let activation = if self.state.settings.drop_zone_expanded {
                edge_autoscroll::activation_at(bounds, pos)
            } else { edge_autoscroll::activation_at_horizontal(bounds, pos) };
            if let Some(activation) = activation { return edge_autoscroll::tick_handle(&handle, activation, elapsed); }
            return false;
        }
        if let Some((bounds, handle)) = self.sidebar_recent_scroll_region.clone() {
            if contains(bounds, pos) && self.tick_pixel_edge_scroll(&handle, bounds, pos, elapsed) { return true; }
        }
        if let Some((bounds, handle)) = self.sidebar_scroll_region.clone() {
            if contains(bounds, pos) {
                return self.tick_pixel_edge_scroll(&handle, bounds, pos, elapsed);
            }
        }
        false
    }

    /// Edge-scroll a pane. All views (grid/list/details) drive a `uniform_list`
    /// whose underlying `base_handle` is a plain `ScrollHandle`, so we pixel-scroll
    /// it directly — the same proven path as the sidebar. The earlier row-based
    /// `scroll_to_item_strict` approach was flaky (deferred, sometimes a no-op).
    fn tick_pane_edge_scroll(
        &mut self,
        pane: TileId,
        bounds: gpui::Bounds<gpui::Pixels>,
        pos: gpui::Point<gpui::Pixels>,
        elapsed: f32,
    ) -> bool {
        let Some(handle) = self.scroll_handles.get(&pane) else {
            return false;
        };
        let base = handle.0.borrow().base_handle.clone();
        // Column-major List scrolls horizontally: dragging to its left/right edge
        // reveals more columns.
        let moved = if self.list_column_step(pane).is_some() {
            crate::ui::edge_autoscroll::activation_at_horizontal(bounds, pos)
                .is_some_and(|activation| {
                    crate::ui::edge_autoscroll::tick_handle(&base, activation, elapsed)
                })
        } else {
            self.tick_pixel_edge_scroll(&base, bounds, pos, elapsed)
        };
        if moved {
            // Separate PaneSignal entity: the canvas repaints App, not the pane.
            self.dirty_panes.insert(pane);
            true
        } else {
            false
        }
    }

    /// Pixel-based edge-scroll for a plain `ScrollHandle` (sidebar + pane base
    /// handle). gpui's scroll offset y is negative going down, so down subtracts.
    fn tick_pixel_edge_scroll(
        &mut self,
        handle: &gpui::ScrollHandle,
        bounds: gpui::Bounds<gpui::Pixels>,
        pos: gpui::Point<gpui::Pixels>,
        elapsed: f32,
    ) -> bool {
        let Some(activation) = edge_autoscroll::activation_at(bounds, pos) else {
            return false;
        };
        edge_autoscroll::tick_handle(handle, activation, elapsed)
    }

    fn execute_drag_hover_command(&mut self, command: DragHoverCommand) {
        match command {
            DragHoverCommand::NavigatePath(path) => {
                let tile = self.state.focused_tile;
                self.navigate(tile, path);
            }
            DragHoverCommand::NavigatePathIn(tile, path) => self.navigate_entry(tile, path),
            DragHoverCommand::NavigateLocation(loc) => {
                let tile = self.state.focused_tile;
                self.navigate_to(tile, loc, crate::app::nav::NavKind::Push);
            }
            DragHoverCommand::NavigateBack(tile) => self.navigate_back(tile),
            DragHoverCommand::NavigateForward(tile) => self.navigate_forward(tile),
            DragHoverCommand::NavigateUp(tile) => self.navigate_up(tile),
            DragHoverCommand::OpenTag(tag) => {
                let tile = self.state.focused_tile;
                self.open_tag_results(tile, tag);
            }
            DragHoverCommand::Process(action) => self.process_actions(vec![action]),
        }
    }
}

pub(crate) fn drag_hover_recorder(
    id: DragHoverId,
    kind: DragHoverKind,
    command: DragHoverCommand,
    view: &Entity<App>,
) -> impl Fn(gpui::Bounds<gpui::Pixels>, &mut Window, &mut gpui::App) + 'static {
    let view = view.clone();
    move |bounds, window, cx| {
        let bounds = bounds.intersect(&window.content_mask().bounds);
        crate::perf::count("prepaint.drag_hover_target");
        let target = DragHoverTarget {
            id: id.clone(),
            bounds,
            kind,
            command: command.clone(),
            drop_command: None,
        };
        view.update(cx, |this, _| this.register_drag_hover_target(target));
    }
}

pub(crate) fn drag_hover_recorder_with_drop(
    id: DragHoverId,
    kind: DragHoverKind,
    command: DragHoverCommand,
    drop_command: DragHoverDropCommand,
    view: &Entity<App>,
) -> impl Fn(gpui::Bounds<gpui::Pixels>, &mut Window, &mut gpui::App) + 'static {
    let view = view.clone();
    move |bounds, window, cx| {
        let bounds = bounds.intersect(&window.content_mask().bounds);
        crate::perf::count("prepaint.drag_hover_drop_target");
        let target = DragHoverTarget {
            id: id.clone(),
            bounds,
            kind,
            command: command.clone(),
            drop_command: Some(drop_command.clone()),
        };
        view.update(cx, |this, _| this.register_drag_hover_target(target));
    }
}

/// `on_prepaint` recorder that publishes the sidebar's window-space bounds +
/// scroll handle so the drag edge-scroll engine can pixel-scroll it. The sidebar
/// is a separate entity, so it can't ride the per-pane row-based path.
pub(crate) fn sidebar_scroll_recorder(
    view: Entity<App>,
    handle: gpui::ScrollHandle,
) -> impl FnOnce(gpui::Bounds<gpui::Pixels>, &mut Window, &mut gpui::App) + 'static {
    move |bounds, _window, cx| {
        crate::perf::count("prepaint.sidebar_scroll");
        view.update(cx, |this, _| {
            this.sidebar_scroll_region = Some((bounds, handle.clone()));
        });
    }
}

pub(crate) fn drag_hover_layer(view: Entity<App>) -> impl gpui::IntoElement {
    let v = view.clone();
    canvas(
        |_, _, _| {},
        move |_, (), window, cx| {
            let app = v.read(cx);
            let gpui_file_drag = cx.active_drag().is_some_and(|drag| {
                drag.value.is::<gpui::ExternalPaths>() || drag.value.is::<crate::ui::root::DraggedFiles>()
            });
            if gpui_file_drag && !app.drag_hover.active {
                v.update(cx, |this, _| {
                    this.begin_drag_hover();
                    this.drag_hover_set_cursor(window.mouse_position());
                });
            }
            let app = v.read(cx);
            if (app.drag_hover.reference_drag || app.drag_hover.cloud_drag
                || (cfg!(not(windows)) && app.drag_hover.active))
                && !cx.has_active_drag()
            {
                v.update(cx, |this, cx| {
                    this.revert_drag_nav();
                    this.drag_hover.clear();
                    cx.notify();
                });
            }
            let active = v.read(cx).drag_hover_maybe_active();
            let vm = v.clone();
            window.on_mouse_event::<MouseMoveEvent>(move |ev, phase, _win, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let pos = ev.position;
                if vm.read(cx).drag_hover_maybe_active() {
                    vm.update(cx, |this, cx| {
                        this.drag_hover_set_cursor(pos);
                        cx.notify();
                    });
                }
            });
            if active {
                v.update(cx, |this, cx| this.tick_drag_hover(cx));
                crate::perf::event(|| "request_animation_frame drag_hover".to_string());
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .inset_0()
}

/// Build one absolutely-positioned spring-load highlight: a soft fill, an accent
/// ring, and an outer glow (box-shadow). `blur`/`border` are in logical pixels.
fn drag_badge(label: String, pos: gpui::Point<gpui::Pixels>, window: &Window, cx: &gpui::App) -> AnyElement {
    let zoom = f32::from(window.rem_size()) / crate::ui::BASE_PX;
    let viewport = window.viewport_size();
    div().absolute()
        .left(gpui::px((f32::from(pos.x) + 18.0 * zoom).min((f32::from(viewport.width) - 260.0 * zoom).max(0.0))))
        .top(gpui::px((f32::from(pos.y) + 24.0 * zoom).min((f32::from(viewport.height) - 40.0 * zoom).max(0.0))))
        .px_2().py_1().rounded_md().bg(cx.theme().popover)
        .text_color(cx.theme().popover_foreground).text_sm().child(label).into_any_element()
}

/// Destination markers only wrap small targets (folder rows/cells, bookmarks);
/// panes, the dropzone and whole sections get just the cursor badge.
const MAX_MARKED_TARGET_H: f32 = 200.0;

fn glow_rect(
    bounds: gpui::Bounds<gpui::Pixels>,
    fill: gpui::Hsla,
    ring: gpui::Hsla,
    glow: gpui::Hsla,
    blur: f32,
    border: f32,
) -> AnyElement {
    div()
        .absolute()
        .left(bounds.left())
        .top(bounds.top())
        .w(bounds.size.width)
        .h(bounds.size.height)
        .rounded(gpui::px(6.0))
        .bg(fill)
        .border(gpui::px(border))
        .border_color(ring)
        .shadow(vec![gpui::BoxShadow {
            color: glow.into(),
            offset: gpui::point(gpui::px(0.0), gpui::px(0.0)),
            blur_radius: gpui::px(blur),
            spread_radius: gpui::px(1.0),
            inset: false,
        }])
        .into_any_element()
}

fn contains(bounds: gpui::Bounds<gpui::Pixels>, pos: gpui::Point<gpui::Pixels>) -> bool {
    pos.x >= bounds.left()
        && pos.x < bounds.right()
        && pos.y >= bounds.top()
        && pos.y < bounds.bottom()
}

fn hover_target_at(
    targets: &[DragHoverTarget],
    pos: gpui::Point<gpui::Pixels>,
    dropzone: Option<gpui::Bounds<gpui::Pixels>>,
    sidebar_viewport: Option<gpui::Bounds<gpui::Pixels>>,
) -> Option<&DragHoverTarget> {
    if dropzone.is_some_and(|bounds| contains(bounds, pos)) {
        return None;
    }
    let visible_hit = |target: &&DragHoverTarget| {
        contains(target.bounds, pos)
            && (!matches!(target.id, DragHoverId::Sidebar(_))
                || sidebar_viewport.is_none_or(|bounds| contains(bounds, pos)))
    };
    // A cached/scrolled body row can retain bounds overlapping the tab strip.
    // The tab under the cursor owns the drop before any underlying row.
    targets.iter().rev().filter(|target| matches!(target.id, DragHoverId::BreadcrumbMenu(_, _))).find(visible_hit)
        .or_else(|| targets.iter().rev().filter(|target| matches!(target.id, DragHoverId::Tab(_))).find(visible_hit))
        .or_else(|| targets.iter().rev().filter(|target| matches!(target.id, DragHoverId::Breadcrumb(..))).find(visible_hit))
        .or_else(|| targets.iter().rev().filter(|target| !matches!(target.id, DragHoverId::Column(_, _))).find(visible_hit))
        .or_else(|| targets.iter().rev().find(visible_hit))
}

fn drop_command_at(
    targets: &[DragHoverTarget],
    pos: gpui::Point<gpui::Pixels>,
    dropzone: Option<gpui::Bounds<gpui::Pixels>>,
    sidebar_viewport: Option<gpui::Bounds<gpui::Pixels>>,
) -> Option<DragHoverDropCommand> {
    if dropzone.is_some_and(|bounds| contains(bounds, pos)) {
        return Some(DragHoverDropCommand::StageInSidebar);
    }
    hover_target_at(targets, pos, dropzone, sidebar_viewport)
        .and_then(|target| target.drop_command.clone())
}

fn resolved_command_at(
    targets: &[DragHoverTarget],
    pos: gpui::Point<gpui::Pixels>,
    dropzone: Option<gpui::Bounds<gpui::Pixels>>,
    sidebar: Option<gpui::Bounds<gpui::Pixels>>,
    tab_command: impl FnOnce(TileId) -> Option<DragHoverDropCommand>,
    fallback: impl FnOnce() -> Option<DragHoverDropCommand>,
) -> Option<DragHoverDropCommand> {
    if dropzone.is_some_and(|bounds| contains(bounds, pos)) { return Some(DragHoverDropCommand::StageInSidebar); }
    if let Some(target) = hover_target_at(targets, pos, dropzone, sidebar) {
        let command = resolve_drop_command(target, tab_command);
        // A blocked tab owns its region; never route through to the pane below it.
        if command.is_some() || matches!(target.id, DragHoverId::Tab(_)) { return command; }
    }
    fallback()
}

/// Cached tab chrome may keep an older destination after spring navigation.
/// Read the current tab by ID at drop time instead of using the captured path.
fn resolve_drop_command(
    target: &DragHoverTarget,
    tab_command: impl FnOnce(TileId) -> Option<DragHoverDropCommand>,
) -> Option<DragHoverDropCommand> {
    match target.id {
        DragHoverId::Tab(tab) => tab_command(tab),
        _ => target.drop_command.clone(),
    }
}

pub(crate) fn retain_visible_drop_targets(
    targets: &mut Vec<DragHoverTarget>,
    exists: impl Fn(TileId) -> bool,
    pane_visible: impl Fn(TileId) -> bool,
    sidebar_visible: bool,
    inspector_visible: bool,
    menu_visible: bool,
) {
    targets.retain(|target| match &target.id {
        DragHoverId::Sidebar(_) => sidebar_visible,
        DragHoverId::Inspector(_) => inspector_visible,
        DragHoverId::Tab(tab) => exists(*tab),
        DragHoverId::BreadcrumbMenu(tab, _) => menu_visible && exists(*tab) && pane_visible(*tab),
        DragHoverId::Folder(tab, _) | DragHoverId::Column(tab, _) | DragHoverId::Breadcrumb(tab, _) | DragHoverId::Chrome(_, tab) => exists(*tab) && pane_visible(*tab),
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropIntent { Copy, Move, Trash, Tag, Bookmark, Stage, Blocked }

pub(crate) fn drop_is_structurally_blocked(command: &DragHoverDropCommand, paths: &[PathBuf], ctrl: bool, shift: bool) -> bool {
    paths.iter().any(|path| atlas_model::location::is_network_root(path))
        || matches!(command, DragHoverDropCommand::RejectNetworkServer)
        || (matches!(command, DragHoverDropCommand::TransferTo(_))
            && drop_intent(command, paths, ctrl, shift) == DropIntent::Blocked)
}

/// Cheap structural preview. Disk existence, permissions and conflicts are
/// validated by the transfer engine at submission, never on each paint.
fn drop_intent(command: &DragHoverDropCommand, paths: &[PathBuf], ctrl: bool, shift: bool) -> DropIntent {
    if paths.iter().any(|path| atlas_model::location::is_network_root(path)) {
        return DropIntent::Blocked;
    }
    match command {
        DragHoverDropCommand::TransferTo(dest) => {
            #[cfg(windows)]
            if atlas_model::location::network_server_root(dest).is_some() {
                return DropIntent::Blocked;
            }
            let effect = transfer_drop_effect(paths, dest, ctrl, shift);
            let target = drop_path_key(dest);
            let has_valid_source = paths.iter().any(|src| {
                valid_drop_source(src, &drop_path_key(src), &target, effect.is_move())
            });
            if atlas_core::fs::paths::is_recycle_bin_path(dest) || (!paths.is_empty() && !has_valid_source) {
                DropIntent::Blocked
            } else if effect.is_move() { DropIntent::Move } else { DropIntent::Copy }
        }
        DragHoverDropCommand::TrashPaths => DropIntent::Trash,
        DragHoverDropCommand::AssignTag(_) => DropIntent::Tag,
        DragHoverDropCommand::AddBookmarks | DragHoverDropCommand::InsertBookmarks(_) => DropIntent::Bookmark,
        DragHoverDropCommand::StageInSidebar => DropIntent::Stage,
        DragHoverDropCommand::RejectArchive | DragHoverDropCommand::RejectNetworkServer => DropIntent::Blocked,
    }
}

fn valid_drop_source(path: &Path, source: &Path, target: &Path, moving: bool) -> bool {
    !target.starts_with(source)
        && !(moving && source.parent() == Some(target))
        && !atlas_core::fs::paths::is_recycle_bin_path(path)
}

fn drop_path_key(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        // Normalize ordinary DOS/UNC and verbatim spellings with no disk I/O.
        // WSL paths keep case-sensitive components inside their distro.
        if let Some((distro, inner)) = atlas_core::fs::paths::wsl_unc_parts(path) {
            return PathBuf::from(r"\\wsl.localhost\").join(distro.to_lowercase())
                .join(inner.trim_start_matches('/').replace('/', "\\"));
        }
        let Some(text) = path.to_str() else { return path.to_path_buf(); };
        let text = text.to_lowercase();
        if let Some(tail) = text.strip_prefix(r"\\?\unc\") {
            return PathBuf::from(format!(r"\\{tail}"));
        }
        if let Some(tail) = text.strip_prefix(r"\\?\").filter(|tail| tail.as_bytes().get(1) == Some(&b':')) {
            return PathBuf::from(tail);
        }
        PathBuf::from(text)
    }
    #[cfg(not(windows))]
    path.to_path_buf()
}

fn transfer_drop_effect(paths: &[PathBuf], dest: &PathBuf, ctrl: bool, shift: bool) -> crate::dnd::DropEffect {
    if dest.to_string_lossy().starts_with("remote:") {
        crate::dnd::DropEffect::Copy
    } else {
        crate::dnd::effect_with_modifiers(crate::dnd::default_effect_for_paths(paths, dest), ctrl, shift)
    }
}

pub(crate) fn location_drop_command(location: &Location) -> DragHoverDropCommand {
    match location {
        #[cfg(windows)]
        Location::Directory(path) if atlas_model::location::network_server_root(path).is_some() => {
            DragHoverDropCommand::RejectNetworkServer
        }
        Location::Directory(path) => DragHoverDropCommand::TransferTo(path.clone()),
        Location::Tag(tag) => DragHoverDropCommand::AssignTag(*tag),
        Location::RecycleBin => DragHoverDropCommand::TrashPaths,
        Location::Archive { .. } => DragHoverDropCommand::RejectArchive,
        Location::NetworkServer(_) => DragHoverDropCommand::RejectNetworkServer,
        #[cfg(feature = "provider-host")]
        Location::Remote { id, path } => DragHoverDropCommand::TransferTo(crate::app::cloud::remote_sentinel(id, path)),
    }
}

fn folder_location(parent: &Location, path: &std::path::Path) -> Location {
    match parent {
        Location::Archive { archive, .. } => Location::Archive { archive: archive.clone(), inner: path.to_path_buf() },
        #[cfg(feature = "provider-host")]
        Location::Remote { id, .. } => Location::Remote { id: id.clone(), path: path.to_string_lossy().replace('\\', "/") },
        _ => Location::Directory(path.to_path_buf()),
    }
}

pub(crate) fn drop_transfer_with_modifiers(
    paths: Vec<PathBuf>, dest: PathBuf, from_self: bool, ctrl: bool, shift: bool,
) -> atlas_model::ops::TransferRequest {
    use atlas_model::ops::{ConflictPolicy, TransferKind, TransferOrigin, TransferRequest};
    let effect = transfer_drop_effect(&paths, &dest, ctrl, shift);
    TransferRequest {
        sources: paths,
        target_names: None,
        dest,
        kind: if effect.is_move() { TransferKind::Move } else { TransferKind::Copy },
        policy: ConflictPolicy::Ask,
        origin: if from_self { TransferOrigin::DndInternal } else { TransferOrigin::DndExternal },
    }
}

pub(crate) fn stage_paths(staged: &mut Vec<atlas_model::app_state::StagedDrop>, paths: Vec<PathBuf>, from_self: bool) {
    let mut existing: std::collections::HashSet<_> = staged.iter().map(|item| item.path.clone()).collect();
    staged.extend(paths.into_iter()
        .filter(|path| path.exists() && !atlas_core::fs::paths::is_recycle_bin_path(path))
        .filter(|path| existing.insert(path.clone()))
        .map(|path| atlas_model::app_state::StagedDrop { path, from_self }));
}

fn consume_staged_paths(staged: &mut Vec<atlas_model::app_state::StagedDrop>, completed: &std::collections::HashSet<PathBuf>) {
    staged.retain(|item| !completed.contains(&item.path));
}

#[cfg(test)]
mod tests {
    #[test]
    fn explicit_shelf_intent_ignores_source_volume_and_retains_unfinished_items() {
        use atlas_model::{app_state::StagedDrop, ops::{ConflictPolicy, TransferKind}};
        use std::path::PathBuf;
        for dest in [r"C:\destination", r"D:\destination"] {
            let paths = vec![PathBuf::from(r"C:\source\one.txt"), PathBuf::from(r"D:\source\two.txt")];
            for move_all in [false, true] {
                let request = super::drop_transfer_with_modifiers(paths.clone(), dest.into(), true, !move_all, move_all);
                assert_eq!(request.kind, if move_all { TransferKind::Move } else { TransferKind::Copy });
                assert_eq!(request.policy, ConflictPolicy::Ask);
                let mut staged: Vec<_> = paths.iter().map(|path| StagedDrop { path: path.clone(), from_self: true }).collect();
                // A cancellation with no completions consumes nothing.
                super::consume_staged_paths(&mut staged, &Default::default());
                assert_eq!(staged.len(), 2);
                // A partial completion consumes only its successful source.
                super::consume_staged_paths(&mut staged, &[paths[0].clone()].into());
                assert_eq!(staged.len(), 1);
                assert_eq!(staged[0].path, paths[1]);
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn native_description_uses_child_live_tab_and_blocked_drop_routing() {
        let pane = TileId(5);
        let child = PathBuf::from("C:/destination/child");
        let mut row = sidebar_row("child", rect(40.0, 30.0));
        row.id = DragHoverId::Folder(pane, child.clone());
        row.drop_command = Some(DragHoverDropCommand::TransferTo(child.clone()));
        let mut snapshot = NativeDropSnapshot {
            targets: vec![row, tab_target(pane, "C:/stale".into())], dropzone: None, sidebar: None,
            tabs: [(pane, DragHoverDropCommand::TransferTo("C:/live".into()))].into(),
            panes: vec![(rect(0.0, 300.0), DragHoverDropCommand::TransferTo("C:/destination".into()))],
        };
        let paths = [PathBuf::from("C:/source/file")];
        assert_eq!(snapshot.transfer_at(100.0, 50.0, &paths, true, false), Some(child.clone()));
        assert_eq!(snapshot.transfer_at(100.0, 18.0, &paths, true, false), Some("C:/live".into()));
        assert_eq!(snapshot.transfer_at(100.0, 50.0, &[child.clone()], true, false), None);
        snapshot.tabs.remove(&pane);
        assert!(snapshot.command_at(100.0, 18.0).is_none());
        assert!(snapshot.command_at(10000.0, 10000.0).is_none());
    }
    #[test]
    fn drop_badge_and_request_use_the_same_modifier_policy() {
        use super::*;
        let paths = vec![PathBuf::from("C:/source/file.txt")];
        let dest = PathBuf::from("C:/destination");
        let command = DragHoverDropCommand::TransferTo(dest.clone());
        for (ctrl, shift) in [(false, false), (true, false), (false, true), (true, true)] {
            let request = drop_transfer_with_modifiers(paths.clone(), dest.clone(), true, ctrl, shift);
            let intent = drop_intent(&command, &paths, ctrl, shift);
            assert_eq!(intent == DropIntent::Move, request.kind == atlas_model::ops::TransferKind::Move);
        }
    }

    #[test]
    fn nested_and_noop_drops_are_rejected_but_mixed_payloads_keep_valid_items() {
        use super::*;
        let source = PathBuf::from("C:/parent/folder");
        let nested = DragHoverDropCommand::TransferTo(source.join("child"));
        assert!(drop_is_structurally_blocked(&nested, &[source.clone()], true, false));
        assert!(!drop_is_structurally_blocked(&nested, &[source.clone(), PathBuf::from("C:/other/file")], true, false));
        let same_parent = DragHoverDropCommand::TransferTo(PathBuf::from("C:/parent"));
        assert!(drop_is_structurally_blocked(&same_parent, &[source.clone()], false, true));
        assert!(!drop_is_structurally_blocked(&same_parent, &[source], true, false));
    }
    use super::*;
    use gpui::{point, px, size, Bounds, Pixels};

    #[test]
    fn provisional_navigation_stays_frozen_across_target_changes_until_drop_or_cancel() {
        let now = Instant::now();
        let mut state = DragHoverState {
            active: true,
            ..Default::default()
        };
        assert!(state.navigation_is_provisional());
        state.reset_target(Some(DragHoverId::Sidebar("recent".into())), now);
        assert!(state.navigation_is_provisional());
        state.reset_target(None, now);
        assert!(state.navigation_is_provisional());
        state.committed = true;
        assert!(!state.navigation_is_provisional());
        state.clear();
        assert!(!state.navigation_is_provisional());
        assert!(!state.committed);
        assert!(state.nav_snapshot.is_none());
    }

    fn rect(y: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(y)), size(px(240.0), px(height)))
    }

    fn sidebar_row(id: &str, bounds: Bounds<Pixels>) -> DragHoverTarget {
        DragHoverTarget {
            id: DragHoverId::Sidebar(id.into()),
            bounds,
            kind: DragHoverKind::Nav,
            command: DragHoverCommand::NavigatePath(PathBuf::from(id)),
            drop_command: Some(DragHoverDropCommand::TransferTo(PathBuf::from(id))),
        }
    }

    #[test]
    fn dropzone_stages_instead_of_activating_overlapping_sidebar_rows() {
        let targets = [sidebar_row("drive", rect(640.0, 120.0))];
        let dropzone = Some(rect(660.0, 76.0));
        // Even stale row/viewport geometry that extends under the shelf cannot
        // navigate, assign a tag or transfer a drop to an underlying location.
        let viewport = Some(rect(0.0, 780.0));
        let pos = point(px(100.0), px(700.0));
        assert!(hover_target_at(&targets, pos, dropzone, viewport).is_none());
        assert!(matches!(
            drop_command_at(&targets, pos, dropzone, viewport),
            Some(DragHoverDropCommand::StageInSidebar)
        ));
    }

    #[test]
    fn clipped_sidebar_rows_cannot_receive_hover_or_drop() {
        let targets = [sidebar_row("bookmark", rect(500.0, 120.0))];
        let viewport = Some(rect(0.0, 550.0));
        let pos = point(px(100.0), px(580.0));
        assert!(hover_target_at(&targets, pos, None, viewport).is_none());
        assert!(drop_command_at(&targets, pos, None, viewport).is_none());

        let visible_pos = point(px(100.0), px(520.0));
        assert!(matches!(
            drop_command_at(&targets, visible_pos, None, viewport),
            Some(DragHoverDropCommand::TransferTo(path)) if path == PathBuf::from("bookmark")
        ));
    }

    #[test]
    fn topmost_visible_target_keeps_priority_outside_dropzone() {
        let targets = [
            sidebar_row("underneath", rect(0.0, 60.0)),
            sidebar_row("topmost", rect(0.0, 60.0)),
        ];
        let pos = point(px(100.0), px(30.0));
        let dropzone = Some(rect(660.0, 76.0));
        assert!(matches!(
            drop_command_at(&targets, pos, dropzone, Some(rect(0.0, 550.0))),
            Some(DragHoverDropCommand::TransferTo(path)) if path == PathBuf::from("topmost")
        ));
    }

    fn tab_target(tab: TileId, destination: PathBuf) -> DragHoverTarget {
        DragHoverTarget {
            id: DragHoverId::Tab(tab),
            bounds: rect(0.0, 36.0),
            kind: DragHoverKind::Nav,
            command: DragHoverCommand::Process(Action::FocusTab(tab)),
            drop_command: Some(DragHoverDropCommand::TransferTo(destination)),
        }
    }

    #[test]
    fn idle_renders_keep_cached_tabs_and_remove_closed_tabs_and_rows() {
        let live = TileId(1);
        let mut targets = vec![
            tab_target(live, "live".into()),
            tab_target(TileId(2), "closed".into()),
            sidebar_row("row", rect(0.0, 60.0)),
        ];
        retain_visible_drop_targets(&mut targets, |tab| tab == live, |_| true, false, false, false);
        assert_eq!(targets.len(), 1);
        let target = hover_target_at(&targets, point(px(100.0), px(18.0)), None, None).unwrap();
        assert_eq!(target.id, DragHoverId::Tab(live));
        // A tab that no longer accepts drops must not use its captured folder.
        assert!(resolve_drop_command(target, |_| None).is_none());
    }

    #[test]
    fn tab_header_wins_over_stale_body_geometry() {
        let tab = TileId(1);
        let targets = vec![tab_target(tab, "tab-folder".into()), sidebar_row("stale-body", rect(0.0, 60.0))];
        let hit = hover_target_at(&targets, point(px(100.0), px(18.0)), None, None).unwrap();
        assert_eq!(hit.id, DragHoverId::Tab(tab));
    }

    #[test]
    fn cached_geometry_survives_idle_but_hidden_owners_are_pruned() {
        let pane = TileId(1);
        let mut folder = sidebar_row("folder", rect(40.0, 30.0));
        folder.id = DragHoverId::Folder(pane, "folder".into());
        let mut menu = sidebar_row("menu", rect(0.0, 30.0));
        menu.id = DragHoverId::BreadcrumbMenu(pane, "menu".into());
        let mut inspector = sidebar_row("inspector", rect(0.0, 30.0));
        inspector.id = DragHoverId::Inspector("inspector".into());
        let mut targets = vec![tab_target(pane, "tab".into()), folder, sidebar_row("sidebar", rect(0.0, 30.0)), menu, inspector];
        retain_visible_drop_targets(&mut targets, |_| true, |_| true, true, true, true);
        assert_eq!(targets.len(), 5, "OLE may release between cached paints; retain all visible owners");
        retain_visible_drop_targets(&mut targets, |_| true, |_| false, false, false, false);
        assert_eq!(targets.len(), 1, "inactive pane bodies and closed panels/popups cannot receive drops");
        assert_eq!(targets[0].id, DragHoverId::Tab(pane));
        retain_visible_drop_targets(&mut targets, |_| false, |_| false, false, false, false);
        assert!(targets.is_empty());
    }

    #[test]
    fn pane_and_tab_location_matrix() {
        let directory = PathBuf::from(r"C:\destination");
        assert!(matches!(location_drop_command(&Location::Directory(directory.clone())), DragHoverDropCommand::TransferTo(path) if path == directory));
        let tag = TagId(7);
        assert!(matches!(location_drop_command(&Location::Tag(tag)), DragHoverDropCommand::AssignTag(id) if id == tag));
        assert!(matches!(location_drop_command(&Location::RecycleBin), DragHoverDropCommand::TrashPaths));
        assert!(matches!(location_drop_command(&Location::Archive { archive: "example.zip".into(), inner: "folder".into() }), DragHoverDropCommand::RejectArchive));
        #[cfg(feature = "provider-host")]
        assert!(matches!(location_drop_command(&Location::Remote { id: "drive".into(), path: "folder".into() }), DragHoverDropCommand::TransferTo(path) if path == PathBuf::from("remote:drive/folder")));
    }

    #[test]
    fn bookmarks_drop_command_variant() {
        let cmd = DragHoverDropCommand::AddBookmarks;
        assert!(matches!(cmd, DragHoverDropCommand::AddBookmarks));
    }

    #[test]
    fn every_filesystem_target_uses_volume_rules_and_modifiers() {
        use atlas_model::ops::{ConflictPolicy, TransferKind};
        for from_self in [false, true] {
            for (dest, ctrl, shift, expected) in [
                (r"C:\destination", false, false, TransferKind::Move),
                (r"D:\destination", false, false, TransferKind::Copy),
                (r"C:\destination", true, false, TransferKind::Copy),
                (r"D:\destination", false, true, TransferKind::Move),
                ("remote:drive/folder", false, true, TransferKind::Copy),
            ] {
                let request = drop_transfer_with_modifiers(vec![r"C:\source\file.txt".into()], dest.into(), from_self, ctrl, shift);
                assert_eq!(request.kind, expected);
                assert_eq!(request.policy, ConflictPolicy::Ask);
            }
        }
    }

    #[test]
    fn virtual_folder_rows_keep_their_provider_and_archive_semantics() {
        let archive = Location::Archive { archive: r"C:\data\example.zip".into(), inner: "parent".into() };
        let folder = folder_location(&archive, std::path::Path::new("parent/child"));
        assert!(matches!(folder, Location::Archive { inner, .. } if inner == PathBuf::from("parent/child")));
        assert!(matches!(location_drop_command(&archive), DragHoverDropCommand::RejectArchive));
        #[cfg(feature = "provider-host")]
        {
            let remote = Location::Remote { id: "drive".into(), path: "parent".into() };
            let folder = folder_location(&remote, std::path::Path::new(r"parent\child"));
            assert!(matches!(location_drop_command(&folder), DragHoverDropCommand::TransferTo(path) if path == PathBuf::from("remote:drive/parent/child")));
        }
    }

    #[test]
    fn clipped_inspector_folder_cannot_receive_a_drop() {
        let mut target = sidebar_row("inspector-folder", rect(500.0, 100.0));
        target.id = DragHoverId::Inspector("inspector-folder".into());
        target.bounds = target.bounds.intersect(&rect(0.0, 540.0));
        let targets = [target];
        assert!(drop_command_at(&targets, point(px(100.0), px(530.0)), None, None).is_some());
        assert!(drop_command_at(&targets, point(px(100.0), px(560.0)), None, None).is_none());
    }

    #[test]
    fn adjacent_targets_have_unambiguous_edges() {
        let targets = [sidebar_row("first", rect(0.0, 30.0)), sidebar_row("second", rect(30.0, 30.0))];
        let hit = hover_target_at(&targets, point(px(100.0), px(30.0)), None, None).unwrap();
        assert_eq!(hit.id, DragHoverId::Sidebar("second".into()));
        assert!(hover_target_at(&targets, point(px(100.0), px(60.0)), None, None).is_none());
    }

    #[test]
    fn miller_folder_row_wins_over_its_column_background() {
        let pane = TileId(1);
        let path = PathBuf::from("same-folder");
        let mut row = sidebar_row("row", rect(40.0, 30.0));
        row.id = DragHoverId::Folder(pane, path.clone());
        let mut column = sidebar_row("column", rect(0.0, 300.0));
        column.id = DragHoverId::Column(pane, path);
        assert_ne!(row.id, column.id);
        let targets = [row, column];
        assert!(matches!(hover_target_at(&targets, point(px(100.0), px(50.0)), None, None).unwrap().id, DragHoverId::Folder(_, _)));
        assert!(matches!(hover_target_at(&targets, point(px(100.0), px(200.0)), None, None).unwrap().id, DragHoverId::Column(_, _)));
    }

    #[test]
    fn cached_tab_drop_transfers_to_its_current_folder_on_disk() {
        use atlas_core::fs::transfer;
        use atlas_model::app_state::BgResult;
        use atlas_model::ops::{ConflictPolicy, TransferKind, TransferOrigin, TransferRequest};
        use std::sync::{atomic::AtomicBool, Arc};

        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("atlas-tab-drop-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        struct TempTree(PathBuf);
        impl Drop for TempTree {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
        let _cleanup = TempTree(root.clone());
        let source_dir = root.join("source");
        let current_dir = root.join("tab-current");
        let old_dir = root.join("tab-before-navigation");
        let active_dir = root.join("other-active-pane");
        for dir in [&source_dir, &current_dir, &old_dir, &active_dir] {
            std::fs::create_dir(dir).unwrap();
        }
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        for kind in [TransferKind::Move, TransferKind::Copy] {
            let name = format!("{kind:?}.txt");
            let source = source_dir.join(&name);
            std::fs::write(&source, b"tab drop payload").unwrap();
            let skipped = source_dir.join(format!("skipped-{name}"));
            std::fs::write(&skipped, b"keep source").unwrap();
            std::fs::write(current_dir.join(skipped.file_name().unwrap()), b"keep destination").unwrap();
            let mut staged = Vec::new();
            stage_paths(&mut staged, vec![source.clone(), source.clone(), skipped.clone()], true);
            stage_paths(&mut staged, vec![source.clone()], false);
            assert_eq!(staged.len(), 2, "duplicates within and across payloads must collapse");
            let tab = TileId(2);
            let mut targets = vec![tab_target(tab, old_dir.clone())];
            // The root renders idle after OLE's Drop; the tab strip stays cached.
            retain_visible_drop_targets(&mut targets, |_| true, |_| true, true, true, false);
            let hit = hover_target_at(&targets, point(px(100.0), px(18.0)), None, None).unwrap();
            let command = resolve_drop_command(hit, |id| {
                assert_eq!(id, tab);
                Some(DragHoverDropCommand::TransferTo(current_dir.clone()))
            }).unwrap();
            let DragHoverDropCommand::TransferTo(dest) = command else { panic!("expected a folder drop"); };
            let request = TransferRequest {
                sources: vec![source.clone(), skipped.clone()], target_names: None, dest,
                kind, policy: ConflictPolicy::SkipAll, origin: TransferOrigin::DndInternal,
            };
            let valid = transfer::validate(&request);
            assert!(valid.fatal.is_none() && valid.errors.is_empty());
            let (conflicts, clean) = transfer::detect_conflicts(&valid.valid, None, &request.dest);
            let built = transfer::build_pairs(&clean, &conflicts, &request.policy, &request.dest);
            let (tx, rx) = flume::unbounded();
            runtime.block_on(transfer::execute(built.pairs, kind, request.origin, 1, tx, Arc::new(AtomicBool::new(false)), vec![]));
            let report = rx.try_iter().find_map(|event| match event {
                BgResult::TransferComplete { report, .. } => Some(report), _ => None,
            }).unwrap();
            assert!(report.errors.is_empty() && !report.cancelled);
            assert_eq!(report.completed, vec![(source.clone(), current_dir.join(&name))]);
            consume_staged_paths(&mut staged, &report.completed.iter().map(|(src, _)| src.clone()).collect());
            assert_eq!(staged.len(), 1);
            assert_eq!(staged[0].path, skipped);
            assert_eq!(std::fs::read(&skipped).unwrap(), b"keep source");
            assert_eq!(std::fs::read(current_dir.join(skipped.file_name().unwrap())).unwrap(), b"keep destination");
            let pairs = transfer::build_pairs(&clean, &conflicts, &ConflictPolicy::RenameAll, &request.dest);
            let (tx, rx) = flume::unbounded();
            runtime.block_on(transfer::execute(pairs.pairs, kind, request.origin, 2, tx, Arc::new(AtomicBool::new(true)), vec![]));
            let report = rx.try_iter().find_map(|event| match event { BgResult::TransferComplete { report, .. } => Some(report), _ => None }).unwrap();
            assert!(report.cancelled && report.completed.is_empty());
            consume_staged_paths(&mut staged, &report.completed.iter().map(|(src, _)| src.clone()).collect());
            assert_eq!(staged.len(), 1, "cancelled operations must preserve staged sources");
            assert_eq!(std::fs::read(current_dir.join(&name)).unwrap(), b"tab drop payload");
            assert_eq!(source.exists(), kind == TransferKind::Copy);
            assert!(!old_dir.join(&name).exists());
            assert!(!active_dir.join(&name).exists());
        }
    }
}
