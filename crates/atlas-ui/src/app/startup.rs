//! `App::new`: construct the app, restore session/profiles, kick off loads.
#![allow(clippy::too_many_lines)] // reason: cohesive startup/drain kept whole

use atlas_model::layout_tree::Tile;
use std::collections::{HashMap, HashSet};

use gpui::{Context, Window};
use smol::Timer;

use super::*;
use atlas_core::search::nucleo_pool::NucleoPool;
use atlas_core::util::config::{load_profiles, load_session};
use atlas_model::app_state::{AppState, BgResult};
use atlas_model::tab::LoadState;

impl App {
    pub fn new(
        rt: tokio::runtime::Handle,
        settings: atlas_model::settings::Settings,
        startup_started: std::time::Instant,
        initial_paths: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_seeded(rt, settings, startup_started, initial_paths, None, window, cx)
    }

    pub(crate) fn new_seeded(
        rt: tokio::runtime::Handle,
        settings: atlas_model::settings::Settings,
        startup_started: std::time::Instant,
        initial_paths: Vec<PathBuf>,
        mut seed: Option<super::windowing::WindowSeed>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let app_new_started = std::time::Instant::now();
        #[cfg(windows)]
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.places_refresh_requested = true;
                cx.notify();
            }
        }).detach();
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some(geometry) = crate::ui::window_geometry::capture(window, cx) else { return; };
            if this.state.settings.window_geometry.as_ref() == Some(&geometry) { return; }
            this.state.settings.window_geometry = Some(geometry);
            if !this.legacy_session_writer { return; }
            // Replacing the task cancels the pending write during live move/resize.
            this.geometry_save_task = Some(cx.spawn(async move |this, cx| {
                Timer::after(std::time::Duration::from_millis(750)).await;
                let snapshot = this.update(cx, |this, _| this.state.settings.clone()).ok();
                if let Some(settings) = snapshot {
                    let ticket = atlas_core::util::config::settings_write_ticket();
                    cx.background_executor().spawn(async move {
                        atlas_core::util::config::save_settings_if_current(&settings, ticket);
                    }).await;
                }
            }));
        }).detach();
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let rubber_escape_listener = cx.listener(|this, ev: &gpui::KeystrokeEvent, window, cx| {
            if ev.keystroke.key == "escape" && (this.rubber.is_some() || this.pending_folder_click.is_some()) {
                let pane = this.rubber.as_ref().and_then(|r| match r.target {
                    RubberTarget::Pane(pane) | RubberTarget::MillerColumn(pane, _) => Some(pane),
                    RubberTarget::Dropzone => None,
                });
                this.cancel_rubber();
                if let Some(pane) = pane {
                    this.notify_pane(pane, cx);
                }
                cx.notify();
                window.refresh();
                cx.stop_propagation();
            }
        });
        let rubber_escape_sub = cx.intercept_keystrokes(rubber_escape_listener);
        let mut profiles = seed.as_mut().and_then(|seed| seed.profiles.take()).unwrap_or_else(load_profiles);
        let legacy_session_writer = seed.as_ref().is_none_or(|seed| seed.legacy_session_writer);
        let skip_listing = seed.as_ref().is_some_and(|seed| seed.skip_listing);
        tracing::info!(
            total_ms = startup_started.elapsed().as_millis(),
            phase_ms = app_new_started.elapsed().as_millis(),
            "startup: profiles"
        );

        let (tx, incoming_rx) = flume::unbounded();
        let stats_writer = if let Some(writer) = seed.as_mut().and_then(|seed| seed.stats_writer.take()) {
            let _ = tx.send(BgResult::FolderStatsLoaded(None));
            writer
        } else {
            let (stats_writer, stats_rx) = flume::bounded(1);
            let stats_tx = tx.clone();
            rt.spawn_blocking(move || {
                let started = std::time::Instant::now();
                let path = atlas_core::fs::stats_cache::path();
                let disk = path.as_deref().and_then(atlas_core::fs::stats_cache::load);
                tracing::info!(elapsed_ms = started.elapsed().as_millis(), entries = disk.as_ref().map_or(0, |d| d.entries.len()), "startup: folder statistics cache loaded (background)");
                let _ = stats_tx.send(BgResult::FolderStatsLoaded(disk));
                atlas_core::fs::stats_cache::writer_loop(path, stats_rx);
            });
            stats_writer
        };
        #[cfg(windows)]
        {
            let volume_tx = tx.clone();
            rt.spawn_blocking(move || {
                let _ = volume_tx.send(BgResult::VolumeSnapshot(atlas_win_space::storage::mounted_volume_serials()));
            });
        }
        let (drain_tx, rx) = flume::unbounded();
        let (icon_tx, incoming_icon_rx) = flume::unbounded();
        let (drain_icon_tx, icon_rx) = flume::unbounded();
        let wake_rx = crate::ui_wake::subscribe();
        cx.observe_self(|_, _| crate::ui_wake::wake()).detach();
        cx.on_release(|app, _| {
            app.restored_tabs = Default::default();
            #[cfg(windows)]
            if let Some(worker) = &app.shell_menu {
                worker.shutdown();
            }
            crate::ui_wake::wake();
        }).detach();
        cx.observe_global::<gpui_component::ThemeRegistry>(|this, cx| {
            this.theme_dirty = true;
            cx.notify();
        }).detach();
        // Forward off the foreground executor: the retained ingress queues must
        // remain readable by the OLE modal-loop drag-hover drain as well.
        cx.background_executor()
            .spawn(async move {
                while let Ok(result) = incoming_rx.recv_async().await {
                    if drain_tx.send(result).is_err() {
                        break;
                    }
                    crate::ui_wake::wake();
                }
            })
            .detach();
        cx.background_executor()
            .spawn(async move {
                while let Ok(result) = incoming_icon_rx.recv_async().await {
                    if drain_icon_tx.send(result).is_err() {
                        break;
                    }
                    crate::ui_wake::wake();
                }
            })
            .detach();

        // Restore previous session tree if available, else create fresh state.
        // If initial_paths were provided (e.g. CLI path or "open in new window"),
        // start a clean session focused on that location rather than restoring older tabs.
        let session_started = std::time::Instant::now();
        let saved_session = seed.as_mut().and_then(|seed| seed.session.take());
        let focused = saved_session.as_ref().map(|session| session.focused);
        let workspace = saved_session.as_ref().and_then(|session| session.workspace.clone());
        if let Some(profile) = saved_session.as_ref().and_then(|session| session.profile.as_ref()) {
            if let Some(index) = profiles.profiles.iter().position(|candidate| &candidate.id == profile) {
                profiles.active_idx = index;
            }
        }
        let restored_tree = saved_session.map(|session| session.tree).or_else(|| {
            (seed.is_none() && initial_paths.is_empty()).then(load_session).flatten()
        });
        let (mut state, _initial_tile_id) = if let Some(tree) = restored_tree {
            let (s, id) = AppState::from_tree(tree, settings);
            // Discard a corrupted/empty session and start fresh
            if s.all_pane_tile_ids().is_empty() {
                AppState::new(s.settings)
            } else {
                (s, id)
            }
        } else {
            AppState::new(settings)
        };
        let mut explicit_startup_pane = None;
        if let Some(path) = initial_paths.first().and_then(|path| {
            #[cfg(windows)]
            if atlas_model::location::network_server_root(path).is_some() {
                return Some(path.clone());
            }
            std::path::absolute(path).ok()
        }) {
            let tile = state.focused_tile;
            if let Some(Tile::Pane(tab)) = state.pane_tree.tiles.get_mut(tile) {
                tab.location = atlas_model::location::Location::from_path(
                    atlas_core::fs::paths::normalize_wsl_unc_path(path),
                );
                tab.volume_serial = None;
                explicit_startup_pane = Some(tile);
            }
        }
        if let Some(focused) = focused.filter(|id| matches!(state.pane_tree.tiles.get(*id), Some(Tile::Pane(_)))) {
            state.focused_tile = focused;
            state.pane_tree.make_active(|id, _| id == focused);
        }
        super::windowing::hide_remote_locations(&mut state.pane_tree);
        state.profiles = profiles;
        if let Some(mut settings) = state.profiles.active().prefs.get::<atlas_model::settings::Settings>(atlas_model::profiles::SETTINGS_PREFS_KEY) {
            settings.window_geometry = state.settings.window_geometry.clone();
            state.settings = settings;
        }
        if let Some(workspace) = workspace {
            state.settings.apply_workspace(&workspace);
        }
        if !skip_listing {
            let folder_views = atlas_core::util::config::folder_view_prefs(state.profiles.active());
            for (_, tile) in state.pane_tree.tiles.iter_mut() {
                if let Tile::Pane(tab) = tile {
                    super::nav::apply_folder_view_pref(tab, &folder_views);
                }
            }
        }
        migrate_global_recent_folders(&mut state);
        if !state.settings.getting_started_seen {
            state.overlay.show_getting_started = true;
        }
        tracing::info!(
            total_ms = startup_started.elapsed().as_millis(),
            phase_ms = session_started.elapsed().as_millis(),
            "startup: session"
        );
        // Resolve the startup icon theme from the active profile's saved selection
        // (official per-profile store), falling back to VSCode auto-discovery.
        let icon_profile = state.profiles.active().id.clone();
        let icon_map = state.profiles.active().scoped::<crate::app::profiles::IconSelection>();
        cx.spawn(async move |this, cx| {
            let expected_map = icon_map.clone();
            let theme = cx.background_executor().spawn(async move {
                let started = std::time::Instant::now();
                let theme = if icon_map.is_empty() { crate::icon_theme::find_vscode_theme() }
                else { crate::icon_theme::IconThemeIndex::from_mappings(icon_map.into_iter()
                    .map(|(ext, path)| (ext, std::path::PathBuf::from(path))).collect()) };
                tracing::info!(phase_ms = started.elapsed().as_millis(), "startup: icon theme (background)");
                theme
            }).await;
            let _ = this.update(cx, |this, cx| {
                if this.state.profiles.active().id == icon_profile
                    && this.state.profiles.active().scoped::<crate::app::profiles::IconSelection>() == expected_map {
                    this.icon_theme = theme;
                    this.icon_cache.clear();
                    this.notify_all_panes(cx);
                    cx.notify();
                }
            });
        }).detach();
        let icon_theme = None;

        // Spawn the SQLite tag-db worker. On a brand-new database it imports the tags
        // that previously lived only in profile config (global + per-profile).
        let migration = collect_tag_migration(&state.profiles);
        let tag_db_tx = atlas_core::fs::tag_db::spawn(
            &rt,
            atlas_core::util::config::data_dir().join("tags.db"),
            migration,
            tx.clone(),
        );
        tracing::info!(
            total_ms = startup_started.elapsed().as_millis(),
            "startup: tag db spawned"
        );

        let restored_tabs = if skip_listing {
            Default::default()
        } else {
            super::fs_sync::restore::RestoredTabs::new(&state, explicit_startup_pane)
        };
        if !skip_listing {
            // Tag definitions and results retain the existing database routing.
            for (tile_id, tile) in state.pane_tree.tiles.iter() {
                let Tile::Pane(tab) = tile else { continue };
                if let atlas_model::location::Location::Tag(tag) = tab.location {
                    let op_id = state.next_op_id();
                    let runtime = state.tab_states.entry(*tile_id).or_default();
                    runtime.load_state = LoadState::Loading;
                    runtime.last_nav_op_id = op_id;
                    let _ = tag_db_tx.send(atlas_core::fs::tag_db::TagDbCmd::QueryPathsForTag(tag));
                }
            }
        }

        // FileOP is unrestricted and performs no commercial entitlement refresh.

        // ── Windows OLE drag-and-drop setup ────────────────────────────────
        // Extract the HWND from gpui's HasWindowHandle impl. Stored as isize
        // so it is Send/clone-able; reconstructed to HWND only in dnd::windows.
        #[cfg(windows)]
        let (hwnd_opt, dnd_opt) = {
            use raw_window_handle::HasWindowHandle;
            let hwnd_raw = window.window_handle().ok().and_then(|h| {
                if let raw_window_handle::RawWindowHandle::Win32(w) = h.as_raw() {
                    Some(w.hwnd.get())
                } else {
                    None
                }
            });
            let dnd = hwnd_raw.and_then(|h| {
                // HiDPI scale from gpui (no cx needed for scale_factor).
                let scale = window.scale_factor();
                crate::dnd::windows::setup_dnd(h, scale)
            });
            (hwnd_raw, dnd)
        };

        // Pre-warm the shell-menu STA worker so the first right-click is instant.
        #[cfg(windows)]
        let shell_menu = Some(crate::shell_menu::spawn());
        #[cfg(windows)]
        if let Some(handle) = &shell_menu {
            handle.prewarm();
        }

        // Extensions: retire official ones whose features are now built in
        // (moved to extensions/.trash, never deleted, never fatal), seed the
        // bundled ones, load enabled overrides and build the registry.
        let inherited_registry = seed.as_mut().and_then(|seed| seed.extensions.take());
        if inherited_registry.is_none() {
            let retired = atlas_core::ext::install::retire_bundled();
            if !retired.is_empty() {
                tracing::info!("retired bundled extensions: {retired:?}");
            }
            let _ = atlas_core::ext::install::ensure_bundled(
                "media-thumbs",
                crate::app::ext::MEDIA_THUMBS_MANIFEST,
            );
            let _ = atlas_core::ext::install::ensure_asset(
                "media-thumbs",
                "icon.svg",
                crate::app::ext::MEDIA_THUMBS_ICON,
            );
            let _ = atlas_core::ext::install::ensure_bundled(
                "icon-manager",
                crate::app::ext::ICON_MANAGER_MANIFEST,
            );
            let _ = atlas_core::ext::install::ensure_asset(
                "icon-manager",
                "icon.svg",
                crate::app::ext::ICON_MANAGER_ICON,
            );
            let runtime_tools_src =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions/runtime-tools");
            let _ = atlas_core::ext::install::ensure_bundled(
                "conversion-pandoc",
                crate::app::ext::CONVERSION_PANDOC_MANIFEST,
            );
            let _ = atlas_core::ext::install::ensure_bundled(
                "conversion-opus",
                crate::app::ext::CONVERSION_OPUS_MANIFEST,
            );
            let _ = atlas_core::ext::install::ensure_bundled_with_assets(
                "runtime-tools",
                crate::app::ext::RUNTIME_TOOLS_MANIFEST,
                &runtime_tools_src,
            );
        }
        state.ext.enabled = atlas_core::util::config::load_ext_enabled();
        let ext_started = std::time::Instant::now();
        let ext_registry = inherited_registry.unwrap_or_else(|| super::ext::visible_registry(&state.ext.enabled));
        tracing::info!(
            total_ms = startup_started.elapsed().as_millis(),
            phase_ms = ext_started.elapsed().as_millis(),
            "startup: extensions"
        );

        // Keep the provider API available without starting its worker or
        // loading connection configuration in this release.
        #[cfg(feature = "provider-host")]
        let (cloud_tx, _) = flume::unbounded();

        // Channels and input wake this task. Timers exist only for active
        // animations, queries, caret edges and toast deadlines.
        cx.spawn(async move |this, cx| {
            let mut delay = Some(Duration::ZERO);
            loop {
                smol::future::or(
                    async {
                        let _ = wake_rx.recv_async().await;
                    },
                    async {
                        if let Some(delay) = delay {
                            Timer::after(delay).await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    },
                )
                .await;
                let alive = this
                    .update(cx, |app, cx| {
                        let mut dirty = app.drain_bg_results(cx);
                        dirty |= app.tick_ctx_input_blink();
                        dirty |= app.tick_batch_blink(cx);
                        // Drive matching only until this query's snapshot is ready;
                        // streamed items must not repaint an already published list.
                        if app.state.overlay.show_goto_bar
                            && !app.state.profiles.active().search_in_content
                            && !app.state.overlay.goto_state.query.trim().is_empty()
                            && app.goto_snapshot.is_pending()
                        {
                            dirty |= app.nucleo.tick();
                        }
                        // Drain Windows OLE drop payloads enqueued by our IDropTarget.
                        #[cfg(windows)]
                        {
                            dirty |= app.drain_dnd_queue();
                            dirty |= app.drain_shell_menu_events(cx);
                        }
                        // Finalize a palette's exit animation once its fade-out has elapsed.
                        dirty |= app.tick_palette_close();
                        // Finalize pane close-out animations: queue their dock removal.
                        dirty |= app.tick_pane_close();
                        dirty |= app.tick_tab_close();
                        dirty |= app.tick_toasts();
                        if dirty {
                            cx.notify();
                        }
                        delay = app.maintenance_delay();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        // Restore per-profile Details column widths + order (falls back to defaults
        // when the active profile has never customized them).
        let split_details_prefs = crate::app::profiles::SplitDetailsColumns::for_profile(state.profiles.active());
        let details_prefs = state
            .profiles
            .active()
            .scoped::<crate::app::profiles::DetailsColumns>();

        let nucleo = NucleoPool::new();
        let goto_root = state
            .active_tab_data()
            .and_then(|td| td.location.fs_dir().map(std::path::Path::to_path_buf))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        if legacy_session_writer {
            atlas_core::fs::index::build_index(goto_root.clone(), nucleo.injector());
        }
        // An installed search service has its own OS lifecycle. In-app folder
        // index above remains available without installing or launching a daemon.

        // transfer_journal resume is WIP and not wired into atlas-core yet.

        #[cfg(windows)]
        if let Some(crash) = atlas_win_space::crash::previous_crash() {
            tracing::warn!(summary = ?crash.summary, reports = crash.reports.len(), "previous session crashed");
            state.overlay.op_errors.push_back(atlas_model::overlay::Toast::new(
                rust_i18n::t!("fileop_crash_previous_session").into_owned(),
                atlas_model::overlay::ToastKind::Error,
            ));
        }

        let thumbnail_cache_mb = state.settings.thumbnail_cache_mb;
        let mut app = Self {
            context_info: super::context_info::ContextInfo::with_writer(stats_writer),
            settings_position: None,
            settings_move: None,
            geometry_save_task: None,
            mounted_volumes: Vec::new(),
            network_editor: None,
            comparison: Default::default(),
            #[cfg(windows)]
            discovered_places: Vec::new(),
            #[cfg(windows)]
            places_refresh_requested: true,
            #[cfg(windows)]
            places_refresh_pending: false,
            state,
            startup_started,
            startup_first_render_logged: false,
            startup_listing_logged: false,
            close_after_tab_transfer: false,
            crash_report_sent: false,
            window_requests: Vec::new(),
            legacy_session_writer,
            waiting_for_moved_tab: skip_listing,
            publish_window_preferences: false,
            show_local_transfers: false,
            tx,
            rx,
            rt,
            focus_handle,
            scroll_handles: HashMap::new(),
            miller_scroll_handles: HashMap::new(),
            tree_loading: HashSet::new(),
            watchers: HashMap::new(),
            moved_watchers: HashMap::new(),
            restored_tabs,
            nucleo,
            goto_index_root: legacy_session_writer.then_some((goto_root, false)),
            goto_snapshot: Default::default(),
            goto_service_cache: None,
            goto_service_inflight: None,
            goto_service_failed: None,
            goto_service_generation: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            goto_prefill: None,
            fuzzy_scorer: atlas_core::search::fuzzy::FuzzyScorer::new(),
            palette_scroll: gpui::ScrollHandle::new(),
            ext_registry,
            tag_db_tx,
            #[cfg(feature = "provider-host")]
            cloud_tx,
            // Phase5: preview texture caches + overlay focus start empty.
            preview_textures: HashMap::new(),
            multi_textures: HashMap::new(),
            dir_ring: None,
            tree_rows_cache: HashMap::new(),

            preview_view: HashMap::new(),
            inspector_animating: false,
            last_viewport_size: None,
            window_resize_suppress_until: None,
            preview_drag: None,
            overlay_focus: None,
            batch_rename_field: BatchRenameField::Pattern,
            // Phase4: grid thumbnail cache starts empty.
            thumb_textures: crate::ui::thumbnail_cache::ThumbnailCache::new(thumbnail_cache_mb),
            shell_icon_requests: Default::default(),
            shell_icons: std::collections::HashMap::new(),
            shell_icon_inflight: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            shell_icon_lru: std::collections::VecDeque::new(),
            ext_thumb_inflight: HashSet::new(),
            // Icon theme resolved above from the active profile (selection or
            // VSCode auto-discovery).
            icon_theme,
            icon_cache: HashMap::new(),
            light_surfaces: false,
            icon_tx,
            icon_rx,
            // Phase A: dock built lazily on first render from the restored pane_tree.
            file_panel_cache: HashMap::new(),
            space_lens: HashMap::new(),
            pane_tabs_cache: HashMap::new(),
            pane_tabs_fp: HashMap::new(),
            pane_tabs_bounds: HashMap::new(),
            dock_flip_last_frame: false,
            dock_settling: false,
            sidebar_panel: None,
            delete_owner: atlas_model::places::DeleteOwner::Content,
            inspector_host: None,
            dock_host: None,
            focus_history: std::collections::VecDeque::new(),
            palette_host: None,
            palette_closing: None,
            layout_revision: 0,
            // Phase C: rubber-band selection state.
            rubber: None,
            pending_folder_click: None,
            recent_folder_navigation: None,
            row_rects: std::cell::RefCell::new(HashMap::new()),
            miller_row_rects: std::cell::RefCell::new(HashMap::new()),
            details_hit: HashMap::new(),
            details_drag_press: None,
            details_row_rings: HashMap::new(),
            details_row_texts: Default::default(),
            grid_hit: HashMap::new(),
            grid_hover: None,
            list_hit: HashMap::new(),
            list_hover: None,
            grid_flip_states: std::rc::Rc::new(std::cell::RefCell::new(HashMap::new())),

            details_hover: None,
            miller_column_bounds: HashMap::new(),
            dropzone_selection: Default::default(),
            dropzone_rects: Vec::new(),
            dropzone_bounds: None,
            dropzone_scroll: None,
            dropzone_focused: false,
            dropzone_drag_paths: HashSet::new(),
            dropzone_pending_tags: HashMap::new(),
            operation_feedback: Default::default(),
            hidden_operation_notices: Default::default(),
            toast_stack: Default::default(),
            recycle_bin_full: None,
            recycle_bin_status_inflight: false,
            recycle_bin_status_checked: false,
            recycle_bin_status_dirty: false,
            recycle_bin_icon_dirty: false,
            view_selection_pending: Default::default(),
            tab_reorder_preview: None,
            view_scroll_pending: Default::default(),
            pending_transfers: std::collections::VecDeque::new(),
            pending_pane_zones: std::cell::RefCell::new(Vec::new()),
            address_editor: None,
            parent_entry_focus: HashMap::new(),
            goto_input: None,
            palette_input: None,
            ext_palette_input: None,
            search_inputs: HashMap::new(),
            batch_pattern_input: None,
            batch_replacement_input: None,
            rename_profile_input: None,
            getting_started_profile_input: None,
            create_tag_input: None,
            renaming_profile_idx: None,
            settings_input: None,
            theme_editor: None,
            theme_fork_error: None,
            settings_ui: crate::ui::settings_palette::SettingsUi::default(),
            settings_font_families: None,
            rename_edit: None,
            batch_input_sub: None,
            content_results: Vec::new(),
            content_gen: 0,
            content_cancel: None,
            theme_dirty: true,
            theme_preview_before: None,
            settings_theme_previewing: false,
            view_preview_before: None,
            icon_style_preview_before: None,
            sidebar_width_dirty: true,
            sidebar_render_width: None,
            sidebar_visual_collapsed: false,
            sidebar_width_animating: false,
            #[cfg(windows)]
            hwnd: hwnd_opt,
            #[cfg(windows)]
            dnd: dnd_opt,
            #[cfg(windows)]
            shell_menu,
            dropped_in_background: false,
            sidebar_hit_model: None,
            was_inactive: false,
            inactive_since: None,
            eco_cleared: false,
            converter: None,
            cleanup_panel: None,
            vault: None,
            #[cfg(windows)]
            vault_mount: None,
            ctx_menu: None,
            ctx_menu_seq: 0,
            #[cfg(windows)]
            ctx_native: Default::default(),
            suppress_next_shell_ctx_menu: false,
            type_filter_scroll: gpui::ScrollHandle::new(),
            settings_scroll: gpui::ScrollHandle::new(),
            getting_started_theme_scroll: gpui::ScrollHandle::new(),
            ext_manager_scroll: gpui::ScrollHandle::new(),
            pane_zoom_sliders: HashMap::new(),
            pane_zoom_slider_subs: HashMap::new(),
            open_view_selectors: HashSet::new(),
            view_selector_hover: HashMap::new(),
            preview_editor: None,
            tag_picker_menu: None,
            tag_picker_input: None,
            tag_picker_sub: None,
            _rubber_escape_sub: Some(rubber_escape_sub),
            tag_picker_scroll: gpui::ScrollHandle::new(),
            thumb_wanted: std::sync::Arc::new(std::sync::Mutex::new(HashSet::new())),
            thumb_wanted_by_pane: HashMap::new(),
            native_thumb_inflight: 0,
            pane_animations: HashMap::new(),
            pane_signals: HashMap::new(),
            dirty_panes: HashSet::new(),
            focus_painted: None,
            sidebar_scroll_region: None,
            sidebar_recent_scroll_region: None,
            atlas_launcher_profiles_anchor: gpui::Point::default(),
            pane_layout_bounds: HashMap::new(),
            pane_layout_animations: HashMap::new(),
            dock_root_bounds: None,
            dock_split_animations: HashMap::new(),
            pane_closing: HashMap::new(),
            tab_closing: HashMap::new(),
            pending_column_reflow: HashSet::new(),
            pane_view_transitions: HashMap::new(),
            ext_manager_input: None,
            ext_prompt_inputs: Vec::new(),
            #[cfg(feature = "provider-host")]
            cloud_add_inputs: None,
            #[cfg(feature = "provider-host")]
            rename_remote_input: None,
            pending_rename: None,
            pending_select: None,
            pending_batch_select: None,
            batch_blink_origin: std::time::Instant::now(),
            batch_blink_visible: true,
            col_widths: details_prefs.widths,
            split_details_prefs,
            split_details_active: false,
            typed_selection: None,
            #[cfg(windows)]
            win_e_enabled: atlas_win_space::integration::win_e_enabled().unwrap_or(false),
            preview_delay: Default::default(),
            details_column_order: details_prefs.order,
            col_resize: None,
            col_dragging: None,
            col_drag_x: 0.0,
            col_drag_grab_x: 0.0,
            col_drag_pane: None,
            type_filters: HashMap::new(),
            size_filters: HashMap::new(),
            file_filter_projections: HashMap::new(),
            type_filter_menu: None,
            type_filter_input: None,
            type_filter_sub: None,
            breadcrumb_menu: None,
            breadcrumb_menu_input: None,
            breadcrumb_menu_sub: None,
            size_filter_menu: None,
            drag_hover: crate::ui::drag_hover::DragHoverState::default(),
            pending_drag_hover_targets: Vec::new(),
            active_external_drag: None,
            pane_view_bounds: HashMap::new(),
            dock_linear_bounds: HashMap::new(),
            dock_resize: None,
            sidebar_resize: None,
            inspector_resize: None,
            dirty_all_panes: false,
            command_shortcut_capture: None,
            command_shortcut_conflict: None,
            vim_pending_key: None,
        };
        if let Some(tab) = explicit_startup_pane.filter(|_| !skip_listing) {
            if let Some(atlas_model::location::Location::NetworkServer(server)) =
                app.tab_data_for(tab).map(|data| &data.location)
            {
                app.spawn_list_network_server(tab, server.clone());
            }
        }
        app.load_visible_restored_tabs();
        app
    }
}

fn migrate_global_recent_folders(state: &mut AppState) {
    if state.settings.recent_folders.is_empty() {
        return;
    }
    use atlas_model::profiles::RecentFoldersFeature;
    let legacy = std::mem::take(&mut state.settings.recent_folders);
    for profile in &mut state.profiles.profiles {
        if profile.scoped::<RecentFoldersFeature>().is_empty() {
            profile.set_scoped::<RecentFoldersFeature>(&legacy);
        }
    }
}
