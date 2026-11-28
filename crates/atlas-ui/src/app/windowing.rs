//! Same-process FLDR windows and foreground tab handoff.
use super::App;
use atlas_core::util::config::{save_window_sessions, WindowSession};
use atlas_model::{
    layout_tree::{Tile, TileId, Tree},
    location::Location,
    pane::TabData,
    profiles::ProfileManager,
    settings::{Settings, WindowGeometry},
    tab::{LoadState, TabState},
};
use gpui::{
    point, px, size, AnyWindowHandle, App as GpuiApp, AppContext as _, Context, Entity, Global,
    Pixels, Point, WeakEntity, Window, WindowId, WindowOptions,
};
use gpui_component::{Root, TitleBar};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub(crate) enum WindowRequest {
    Open(Option<PathBuf>),
    Move(TileId),
}

#[derive(Default)]
pub(crate) struct WindowSeed {
    pub session: Option<WindowSession>,
    pub profiles: Option<ProfileManager>,
    pub stats_writer: Option<flume::Sender<atlas_core::fs::stats_cache::WriteRequest>>,
    pub extensions: Option<atlas_core::ext::Registry>,
    pub legacy_session_writer: bool,
    pub skip_listing: bool,
}

#[derive(Clone)]
struct RegisteredWindow {
    handle: AnyWindowHandle,
    view: WeakEntity<App>,
}

type ShutdownSave = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Windows {
    live: Vec<RegisteredWindow>,
    last_closed: Option<WindowSession>,
    shutdown_save: Option<ShutdownSave>,
}
impl Global for Windows {}

static SESSION_REVISION: AtomicU64 = AtomicU64::new(0);

fn session(app: &App) -> Option<WindowSession> {
    if app.close_after_tab_transfer
        || app.waiting_for_moved_tab
        || app.vault.is_some()
        || app.open_pane_count() == 0
    {
        return None;
    }
    let mut tree = app.state.pane_tree.clone();
    for id in app.tab_closing.keys().chain(app.pane_closing.keys()) {
        tree.tiles.remove(*id);
    }
    tree.sanitize();
    Some(WindowSession {
        tree,
        focused: app.state.focused_tile,
        geometry: app.state.settings.window_geometry.clone(),
        profile: Some(app.state.profiles.active().id.clone()),
        workspace: Some(app.state.settings.workspace()),
    })
}

fn snapshots(cx: &GpuiApp) -> Option<Vec<WindowSession>> {
    let windows = cx.try_global::<Windows>()?;
    let views: Vec<_> = windows
        .live
        .iter()
        .filter_map(|w| w.view.upgrade())
        .collect();
    if views.is_empty() {
        return windows.last_closed.clone().map(|session| vec![session]);
    }
    let apps: Vec<_> = views.iter().map(|view| view.read(cx)).collect();
    if apps
        .iter()
        .any(|app| !app.state.settings.autosave_session || app.vault.is_some())
    {
        return None;
    }
    let sessions: Vec<_> = apps.iter().filter_map(|app| session(app)).collect();
    (!sessions.is_empty()).then_some(sessions)
}

fn checkpoint(cx: &mut GpuiApp) {
    if let Some(sessions) = snapshots(cx) {
        let revision = SESSION_REVISION.fetch_add(1, Ordering::Relaxed) + 1;
        cx.background_executor()
            .spawn(async move {
                save_window_sessions(sessions, revision);
            })
            .detach();
    }
}

pub(crate) fn install(cx: &mut GpuiApp) {
    cx.set_global(Windows::default());
    cx.on_window_closed(|cx, id| {
        if let Some(view) = cx.global::<Windows>().live.iter()
            .find(|window| window.handle.window_id() == id)
            .and_then(|window| window.view.upgrade())
        {
            view.update(cx, |app, _| {
                app.restored_tabs = Default::default();
                app.watchers.clear();
                #[cfg(windows)]
                if let Some(worker) = &app.shell_menu {
                    worker.shutdown();
                }
            });
        }
        cx.global_mut::<Windows>()
            .live
            .retain(|window| window.handle.window_id() != id);
        checkpoint(cx);
    })
    .detach();
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(Duration::from_secs(60))
            .await;
        if !cx.update(|cx| {
            let live = !registered(cx).is_empty();
            if live {
                checkpoint(cx);
            }
            live
        }) {
            break;
        }
    })
    .detach();
    cx.on_app_quit(|cx| {
        #[cfg(feature = "conversion-lab")]
        let lab = std::env::var_os("ATLAS_CONVERSION_LAB").is_some();
        #[cfg(not(feature = "conversion-lab"))]
        let lab = false;
        let lab =
            lab || cfg!(feature = "ui-lab") && std::env::var_os("ATLAS_PREVIEW_LAB").is_some();
        let save = if lab {
            None
        } else {
            let views = registered(cx);
            let active = cx.active_window();
            let selected = views
                .iter()
                .find(|window| Some(window.handle) == active)
                .or_else(|| views.first());
            let save = selected
                .and_then(|window| {
                    window
                        .view
                        .update(cx, |app, _| {
                            Box::new(app.prepare_shutdown_save()) as ShutdownSave
                        })
                        .ok()
                })
                .or_else(|| cx.global_mut::<Windows>().shutdown_save.take());
            #[cfg(windows)]
            atlas_win_space::crash::mark_clean_exit();
            save
        };
        let sessions = if lab { None } else { snapshots(cx) };
        let revision = SESSION_REVISION.fetch_add(1, Ordering::Relaxed) + 1;
        let executor = cx.background_executor().clone();
        async move {
            executor
                .spawn(async move {
                    if let Some(save) = save {
                        save();
                    }
                    if let Some(sessions) = sessions {
                        save_window_sessions(sessions, revision);
                    }
                })
                .await;
        }
    })
    .detach();
}

fn registered(cx: &GpuiApp) -> Vec<RegisteredWindow> {
    cx.try_global::<Windows>()
        .map(|windows| windows.live.clone())
        .unwrap_or_default()
}

pub(crate) fn register(view: &Entity<App>, window: &mut Window, cx: &mut GpuiApp) {
    let handle = window.window_handle();
    cx.global_mut::<Windows>().live.push(RegisteredWindow {
        handle,
        view: view.downgrade(),
    });
    let view = view.downgrade();
    window.on_window_should_close(cx, move |window, cx| {
        let last = registered(cx).len() == 1;
        let result = view.update(cx, |app, cx| {
            if !app.close_native_vault(false) {
                return None;
            }
            if let Some(geometry) = crate::ui::window_geometry::capture(window, cx) {
                app.state.settings.window_geometry = Some(geometry);
            }
            let snapshot = (last && app.state.settings.autosave_session)
                .then(|| session(app))
                .flatten();
            if !last {
                app.flush_folder_statistics();
            }
            let save = last.then(|| Box::new(app.prepare_shutdown_save()) as ShutdownSave);
            Some((snapshot, save))
        });
        match result {
            Ok(Some((snapshot, save))) => {
                if last {
                    let windows = cx.global_mut::<Windows>();
                    windows.last_closed = snapshot;
                    windows.shutdown_save = save;
                }
                true
            }
            Ok(None) => false,
            Err(_) => true,
        }
    });
}

pub(crate) fn restore_secondary_windows(
    primary: &Entity<App>,
    sessions: Vec<WindowSession>,
    window: &mut Window,
    cx: &mut GpuiApp,
) {
    if sessions.is_empty() {
        return;
    }
    let primary = primary.downgrade();
    window.on_next_frame(move |_, cx| {
        let Some(primary) = primary.upgrade() else {
            return;
        };
        for session in sessions {
            let app = primary.read(cx);
            let mut seed = app.window_seed();
            let mut settings = app.state.settings.clone();
            settings.window_geometry = session.geometry.clone();
            seed.session = Some(session);
            let rt = app.rt.clone();
            if let Err(error) = open_window(rt, settings, seed, Vec::new(), false, cx) {
                primary.update(cx, |app, cx| {
                    app.toast_error(format!("Could not restore an FLDR window: {error}"));
                    cx.notify();
                });
            }
        }
    });
}

fn open_window(
    rt: tokio::runtime::Handle,
    settings: Settings,
    seed: WindowSeed,
    paths: Vec<PathBuf>,
    focus: bool,
    cx: &mut GpuiApp,
) -> anyhow::Result<RegisteredWindow> {
    let (bounds, display_id) =
        crate::ui::window_geometry::restore(settings.window_geometry.as_ref(), cx);
    let options = WindowOptions {
        show: !seed.skip_listing,
        focus: focus && !seed.skip_listing,
        window_bounds: Some(bounds),
        display_id,
        titlebar: Some(TitleBar::title_bar_options()),
        window_min_size: Some(size(px(640.), px(400.))),
        app_id: Some("digART.FLDR".into()),
        ..Default::default()
    };
    let mut registered_window = None;
    // The WindowHandle is created only if GPUI successfully creates the native window.
    let handle = cx.open_window(options, |window, cx| {
        window.set_window_title(&rust_i18n::t!("app_name"));
        let view = cx
            .new(|cx| App::new_seeded(rt, settings, Instant::now(), paths, Some(seed), window, cx));
        register(&view, window, cx);
        registered_window = Some(RegisteredWindow {
            handle: window.window_handle(),
            view: view.downgrade(),
        });
        cx.new(|cx| Root::new(view, window, cx))
    })?;
    debug_assert!(registered_window.as_ref().is_some_and(
        |window| window.handle.window_id() == AnyWindowHandle::from(handle).window_id()
    ));
    registered_window.ok_or_else(|| anyhow::anyhow!("FLDR did not initialize its new window"))
}

pub(crate) fn hide_remote_locations(tree: &mut Tree<TabData>) {
    #[cfg(feature = "provider-host")]
    for (_, tile) in tree.tiles.iter_mut() {
        let Tile::Pane(tab) = tile else { continue };
        let remote = |location: &Location| matches!(location, Location::Remote { .. });
        tab.history.retain(|location| !remote(location));
        tab.history_forward.retain(|location| !remote(location));
        if remote(&tab.location) {
            tab.location = Location::Directory(dirs::home_dir().unwrap_or_default());
            tab.volume_serial = None;
        }
    }
    #[cfg(not(feature = "provider-host"))]
    let _ = tree;
}

struct MovedTab {
    data: TabData,
    runtime: TabState,
    type_filters: Option<HashSet<String>>,
    size_filters: Option<HashSet<crate::ui::size_filter_menu::SizeFilter>>,
    scroll: Option<Point<Pixels>>,
    miller_scroll: Vec<(usize, Point<Pixels>)>,
    watcher: Option<tokio::task::JoinHandle<()>>,
}

impl App {
    fn window_seed(&self) -> WindowSeed {
        WindowSeed {
            profiles: Some(self.state.profiles.clone()),
            stats_writer: self.context_info.stats_writer.clone(),
            extensions: Some(self.ext_registry.clone()),
            ..Default::default()
        }
    }

    pub(crate) fn dispatch_window_requests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.window_requests.is_empty() {
            return;
        }
        let requests = std::mem::take(&mut self.window_requests);
        let source = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            for request in requests {
                match request {
                    WindowRequest::Move(tab) => {
                        move_tab(source.clone(), tab, None, None, window, cx)
                    }
                    WindowRequest::Open(path) => {
                        let Some(view) = source.upgrade() else { break };
                        let app = view.read(cx);
                        let mut settings = app.state.settings.clone();
                        settings.window_geometry = new_geometry(window, cx);
                        let rt = app.rt.clone();
                        let seed = app.window_seed();
                        if let Err(error) =
                            open_window(rt, settings, seed, path.into_iter().collect(), true, cx)
                        {
                            view.update(cx, |app, cx| {
                                app.toast_error(format!("Could not open a new window: {error}"));
                                cx.notify();
                            });
                        }
                    }
                }
            }
        });
    }

    pub(crate) fn share_window_preferences(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.publish_window_preferences) {
            return;
        }
        let panes = registered(cx);
        if panes.len() < 2 {
            return;
        }
        let source = cx.entity().downgrade();
        let settings = self.state.settings.clone();
        let profile = self.state.profiles.active();
        let (profile_id, theme) = (profile.id.clone(), profile.theme.clone());
        cx.defer(move |cx| {
            for window in panes {
                if window.view == source {
                    continue;
                }
                let _ = window.view.update(cx, |app, cx| {
                    let workspace = app.state.settings.workspace();
                    let geometry = app.state.settings.window_geometry.clone();
                    app.state.settings.clone_from(&settings);
                    app.state.settings.apply_workspace(&workspace);
                    app.state.settings.window_geometry = geometry;
                    if app.state.profiles.active().id == profile_id {
                        app.state.profiles.active_mut().theme.clone_from(&theme);
                    }
                    app.theme_dirty = true;
                    app.notify_all_panes(cx);
                    cx.notify();
                });
            }
        });
    }

    fn can_move_tab(&self, tab: TileId) -> bool {
        #[cfg(windows)]
        if self.vault_mount.is_some() {
            return false;
        }
        self.tab_data_for(tab)
            .is_some_and(|data| !atlas_core::vault::live::is_private(&data.location.display_path()))
            && !self.moved_watchers.contains_key(&tab)
            && self.state.tab_states.contains_key(&tab)
            && !self.tab_closing.contains_key(&tab)
            && !self.pane_closing.contains_key(&tab)
            && self
                .rename_edit
                .as_ref()
                .is_none_or(|edit| edit.pane != tab)
            && self.vault.is_none()
            && (self.open_pane_count() > 1
                || (self.state.ops.active_ops.is_empty()
                    && self.converter.is_none()
                    && self.cleanup_panel.is_none()))
    }

    fn take_moving_tab(&mut self, tab: TileId) -> Option<MovedTab> {
        if !self.can_move_tab(tab) {
            return None;
        }
        let next_focus = if self.state.focused_tile == tab {
            self.adjacent_live_tab_after_closing(tab)
                .or_else(|| self.focus_after_closing(&[tab]))
        } else {
            None
        };
        let runtime = self.state.tab_states.remove(&tab)?;
        let watcher = self
            .watchers
            .remove(&tab)
            .map(super::fs_sync::PaneWatcher::stop);
        let scroll = self
            .scroll_handles
            .remove(&tab)
            .map(|handle| {
                let offset = handle.0.borrow().base_handle.offset();
                offset
            });
        let mut miller_scroll = Vec::new();
        self.miller_scroll_handles.retain(|(pane, index), handle| {
            if *pane != tab {
                return true;
            }
            miller_scroll.push((*index, handle.0.borrow().base_handle.offset()));
            false
        });
        let type_filters = self.type_filters.remove(&tab);
        let size_filters = self.size_filters.remove(&tab);
        let data = self.remove_pane_from_tree_without_reconcile(tab, false)?;
        self.tab_reorder_preview = None;
        if self.open_pane_count() == 0 {
            self.close_after_tab_transfer = true;
        } else {
            self.reconcile_tree_state();
            if let Some(next) = next_focus {
                self.activate_tab(next);
            }
        }
        Some(MovedTab {
            data,
            runtime,
            type_filters,
            size_filters,
            scroll,
            miller_scroll,
            watcher,
        })
    }

    fn receive_moved_tab(&mut self, id: TileId, moved: MovedTab) {
        let MovedTab {
            data,
            mut runtime,
            type_filters,
            size_filters,
            scroll,
            miller_scroll,
            watcher,
        } = moved;
        let location = data.location.clone();
        let view = data.view_mode.clone();
        let reload = location.is_directory()
            || matches!(
                runtime.load_state,
                LoadState::Loading | LoadState::NotLoaded
            );
        if let Some(Tile::Pane(placeholder)) = self.state.pane_tree.tiles.get_mut(id) {
            *placeholder = data;
        }
        runtime.network_listing_pending = false;
        runtime.last_nav_op_id = self.state.next_op_id();
        runtime.path_edit = Default::default();
        runtime.search.focus = false;
        if reload {
            if let Some(base) = runtime.search.original_entries.take() {
                let tree = view == atlas_model::settings::ViewMode::Tree;
                runtime.mutate_preserving_selection(tree, |runtime| runtime.set_entries(base));
            }
        }
        runtime.last_pane_width = 0.0;
        runtime.last_pane_height = 0.0;
        runtime.set_entries(runtime.entries.clone());
        self.state.tab_states.insert(id, runtime);
        self.waiting_for_moved_tab = false;
        if let Some(filters) = type_filters {
            self.type_filters.insert(id, filters);
        }
        if let Some(filters) = size_filters {
            self.size_filters.insert(id, filters);
        }
        if let Some(offset) = scroll {
            self.scroll_handles
                .entry(id)
                .or_default()
                .0
                .borrow()
                .base_handle
                .set_offset(offset);
        }
        for (column, offset) in miller_scroll {
            self.miller_scroll_handles
                .entry((id, column))
                .or_default()
                .0
                .borrow()
                .base_handle
                .set_offset(offset);
        }
        self.reconcile_tree_state();
        self.activate_tab(id);
        // Transfer ordering matters on SMB: the source watcher can take time
        // to close even though its stop signal has already been delivered.
        if let Some(watcher) = watcher {
            let operation = self.state.tab_states[&id].last_nav_op_id;
            self.moved_watchers.insert(id, operation);
            let tx = self.tx.clone();
            self.rt.spawn(async move {
                let _ = watcher.await;
                let _ = tx.send(atlas_model::app_state::BgResult::TabWatcherReleased {
                    tab: id,
                    operation,
                });
            });
        } else if reload {
            self.spawn_list(id, location);
        }
        if view == atlas_model::settings::ViewMode::Miller && reload {
            self.refresh_miller_columns(id);
        }
        self.dirty_panes.insert(id);
    }
}

fn new_geometry(window: &Window, cx: &GpuiApp) -> Option<WindowGeometry> {
    let mut geometry = crate::ui::window_geometry::capture(window, cx)?;
    geometry.x += 30.0;
    geometry.y += 30.0;
    geometry.maximized = false;
    Some(geometry)
}

#[cfg(windows)]
#[allow(unsafe_code)] // Locate the physical cursor's monitor and convert to GPUI display units.
fn tab_drop_geometry(window: &Window, cx: &GpuiApp) -> Option<WindowGeometry> {
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST},
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::GetCursorPos,
        },
    };
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.ok()?;
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    let (mut x, mut y) = (96, 96);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) }.ok()?;
    let mut geometry = new_geometry(window, cx)?;
    geometry.x = cursor.x as f32 * 96.0 / x.max(1) as f32 - 96.0;
    geometry.y = cursor.y as f32 * 96.0 / y.max(1) as f32 - 16.0;
    let id = gpui::DisplayId::new(monitor.0 as u64);
    geometry.monitor = cx
        .displays()
        .into_iter()
        .find(|display| display.id() == id)
        .and_then(|display| display.uuid().ok())
        .map(|uuid| uuid.to_string());
    Some(geometry)
}

struct TabDestination {
    window: RegisteredWindow,
    position: Point<Pixels>,
}

fn move_tab(
    source: WeakEntity<App>,
    tab: TileId,
    destination: Option<TabDestination>,
    placement: Option<WindowGeometry>,
    window: &mut Window,
    cx: &mut GpuiApp,
) {
    let Some(source) = source.upgrade() else {
        return;
    };
    let result = move_tab_inner(&source, tab, destination, placement, window, cx);
    if let Err(error) = result {
        source.update(cx, |app, cx| {
            app.toast_error(error.to_string());
            cx.notify();
        });
    }
}

fn move_tab_inner(
    source: &Entity<App>,
    tab: TileId,
    destination: Option<TabDestination>,
    placement: Option<WindowGeometry>,
    window: &mut Window,
    cx: &mut GpuiApp,
) -> anyhow::Result<()> {
    let app = source.read(cx);
    let data = app
        .tab_data_for(tab)
        .filter(|_| app.can_move_tab(tab))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("The tab cannot be moved while it is closing or busy"))?;

    if let Some(target) = destination {
        let Some(target_view) = target.window.view.upgrade() else {
            anyhow::bail!("The destination window closed");
        };
        anyhow::ensure!(
            target_view != *source,
            "The destination is the source window"
        );
        let handle = target.window.handle;
        handle.update(cx, |_, destination_window, cx| {
            let (id, previous_focus) = target_view
                .update(cx, |app, _| {
                    let pos = target.position;
                    let hit = app
                        .pending_drag_hover_targets
                        .iter()
                        .rev()
                        .find_map(|target| match target.id {
                            crate::ui::drag_hover::DragHoverId::Tab(id)
                                if target.bounds.contains(&pos)
                                    && !app.tab_closing.contains_key(&id)
                                    && !app.pane_closing.contains_key(&id) =>
                            {
                                Some((id, pos.x >= target.bounds.center().x))
                            }
                            _ => None,
                        });
                    let previous_focus = app.state.focused_tile;
                    let anchor = hit
                        .map(|(tab, _)| tab)
                        .or_else(|| {
                            app.state
                                .pane_tree
                                .visible_panes()
                                .into_iter()
                                .find(|pane| {
                                    app.pane_view_bounds
                                        .get(pane)
                                        .is_some_and(|bounds| bounds.contains(&pos))
                                })
                        })
                        .unwrap_or(previous_focus);
                    if app.tab_closing.contains_key(&anchor)
                        || app.pane_closing.contains_key(&anchor)
                    {
                        return None;
                    }
                    let group = app.state.find_tabs_container(anchor);
                    let id = app.insert_tab(anchor, data)?;
                    if let Some((anchor, after)) = hit {
                        if let Some(Tile::Container(atlas_model::layout_tree::Container::Tabs(
                            tabs,
                        ))) = group.and_then(|group| app.state.pane_tree.tiles.get_mut(group))
                        {
                            tabs.children.retain(|child| *child != id);
                            let index = tabs
                                .children
                                .iter()
                                .position(|child| *child == anchor)
                                .unwrap_or(tabs.children.len());
                            tabs.children
                                .insert((index + after as usize).min(tabs.children.len()), id);
                        }
                    }
                    Some((id, previous_focus))
                })
                .ok_or_else(|| anyhow::anyhow!("The destination cannot accept the tab"))?;
            finish_move(
                source,
                tab,
                &target_view,
                id,
                previous_focus,
                window,
                destination_window,
                cx,
            )
        })??;
    } else {
        let mut settings = app.state.settings.clone();
        settings.window_geometry = placement.or_else(|| new_geometry(window, cx));
        let mut seed = app.window_seed();
        let mut tree = Tree::empty();
        let id = tree.tiles.insert_pane(data);
        tree.root = Some(tree.tiles.insert_tab_tile(vec![id]));
        seed.session = Some(WindowSession {
            tree,
            focused: id,
            geometry: settings.window_geometry.clone(),
            profile: Some(app.state.profiles.active().id.clone()),
            workspace: Some(settings.workspace()),
        });
        seed.skip_listing = true;
        let target = open_window(app.rt.clone(), settings, seed, Vec::new(), true, cx)?;
        let Some(target_view) = target.view.upgrade() else {
            anyhow::bail!("The new window closed before accepting the tab");
        };
        let moved = target.handle.update(cx, |_, destination_window, cx| {
            let result = finish_move(
                source,
                tab,
                &target_view,
                id,
                id,
                window,
                destination_window,
                cx,
            );
            if result.is_err() {
                destination_window.remove_window();
            }
            result
        })?;
        moved?;
    }
    checkpoint(cx);
    Ok(())
}

fn finish_move(
    source: &Entity<App>,
    tab: TileId,
    target: &Entity<App>,
    id: TileId,
    previous_focus: TileId,
    window: &mut Window,
    destination_window: &mut Window,
    cx: &mut GpuiApp,
) -> anyhow::Result<()> {
    let Some(moved) = source.update(cx, |app, cx| {
        let tab = app.take_moving_tab(tab)?;
        cx.notify();
        Some(tab)
    }) else {
        target.update(cx, |app, cx| {
            app.remove_pane_from_tree_without_reconcile(id, false);
            app.reconcile_tree_state();
            app.activate_tab(previous_focus);
            cx.notify();
        });
        anyhow::bail!("The source tab changed before it could be moved");
    };
    target.update(cx, |app, cx| {
        app.receive_moved_tab(id, moved);
        app.focus_handle.focus(destination_window, cx);
        cx.notify();
    });
    destination_window.set_visible(true);
    destination_window.activate();
    if source.read(cx).close_after_tab_transfer {
        window.remove_window();
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn release_tab_drag(
    source: &Entity<App>,
    tab: TileId,
    origin: WindowId,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut GpuiApp,
) {
    if window.window_handle().window_id() != origin || cx.try_global::<Windows>().is_none() {
        return;
    }
    let Some(hwnd) = source.read(cx).hwnd else {
        return;
    };
    let target = native_drop_window(hwnd, cx);
    let destination = match target {
        NativeDrop::Source | NativeDrop::Unknown => return,
        NativeDrop::OtherWindow(target) => Some(target),
        NativeDrop::Desktop => {
            let viewport = window.viewport_size();
            let bounds = gpui::Bounds::new(
                point(px(-12.), px(-12.)),
                size(viewport.width + px(24.), viewport.height + px(24.)),
            );
            if bounds.contains(&position) {
                return;
            }
            None
        }
    };
    let placement = if destination.is_none() {
        tab_drop_geometry(window, cx)
    } else {
        None
    };
    let weak = source.downgrade();
    cx.stop_active_drag(window);
    cx.stop_propagation();
    window.defer(cx, move |window, cx| {
        move_tab(weak, tab, destination, placement, window, cx)
    });
}

#[cfg(windows)]
enum NativeDrop {
    Source,
    OtherWindow(TabDestination),
    Desktop,
    Unknown,
}

#[cfg(windows)]
#[allow(unsafe_code)] // Win32 hit testing supplies native z-order and client coordinates.
fn native_drop_window(source: isize, cx: &mut GpuiApp) -> NativeDrop {
    use windows::Win32::{
        Foundation::{HWND, POINT},
        Graphics::Gdi::ScreenToClient,
        UI::WindowsAndMessaging::{GetAncestor, GetCursorPos, WindowFromPoint, GA_ROOT},
    };
    let mut cursor = POINT::default();
    if unsafe { GetCursorPos(&mut cursor) }.is_err() {
        return NativeDrop::Unknown;
    }
    let top = unsafe { GetAncestor(WindowFromPoint(cursor), GA_ROOT) };
    if top.0 == HWND(source as *mut _).0 {
        return NativeDrop::Source;
    }
    let Some(target) = registered(cx).into_iter().find(|target| {
        target
            .view
            .upgrade()
            .is_some_and(|view| view.read(cx).hwnd == Some(top.0 as isize))
    }) else {
        return NativeDrop::Desktop;
    };
    if !unsafe { ScreenToClient(top, &mut cursor) }.as_bool() {
        return NativeDrop::Unknown;
    }
    let scale = target
        .handle
        .update(cx, |_, window, _| window.scale_factor());
    let Ok(scale) = scale else {
        return NativeDrop::Unknown;
    };
    NativeDrop::OtherWindow(TabDestination {
        window: target,
        position: point(px(cursor.x as f32 / scale), px(cursor.y as f32 / scale)),
    })
}
