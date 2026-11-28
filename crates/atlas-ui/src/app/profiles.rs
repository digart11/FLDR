use crate::app::App;
use atlas_model::layout_tree::{Tile, Tree};
use atlas_model::pane::TabData;
use std::path::PathBuf;

impl App {
    pub(crate) fn request_volume_snapshot(&self) {
        #[cfg(windows)]
        {
            let tx = self.tx.clone();
            self.rt.spawn_blocking(move || {
                let _ = tx.send(atlas_model::app_state::BgResult::VolumeSnapshot(
                    atlas_win_space::storage::mounted_volume_serials()));
            });
        }
    }

    pub(crate) fn apply_volume_snapshot(&mut self, volumes: Vec<(PathBuf, u32)>) {
        atlas_core::util::config::set_volume_snapshot(&volumes);
        let mut result = self.state.profiles.reconcile_volume_paths(&volumes);
        let mut session_changed = false;
        let mut navigations = Vec::new();
        for (id, tile) in self.state.pane_tree.tiles.iter_mut() {
            if let Tile::Pane(tab) = tile {
                let old = tab.location.clone();
                let old_path = tab.volume_path().map(std::path::Path::to_path_buf);
                session_changed |= tab.reconcile_volume(&volumes);
                if old != tab.location {
                    if let (Some(old), Some(new)) = (old_path, tab.volume_path()) {
                        result.moved.push((old, new.to_path_buf()));
                    }
                    navigations.push((*id, tab.location.clone()));
                }
            }
        }
        self.mounted_volumes = volumes;
        let focused = self.state.focused_tile;
        for (id, location) in navigations {
            if self.restored_tabs.reschedule(id) {
                self.watchers.remove(&id);
                let folder_views = atlas_core::util::config::folder_view_prefs(self.state.profiles.active());
                if let Some(Tile::Pane(tab)) = self.state.pane_tree.tiles.get_mut(id) {
                    super::nav::apply_folder_view_pref(tab, &folder_views);
                }
                let operation = self.state.next_op_id();
                if let Some(tab) = self.state.tab_states.get_mut(&id) {
                    tab.begin_navigation();
                    tab.last_nav_op_id = operation;
                    tab.load_state = atlas_model::tab::LoadState::NotLoaded;
                }
            } else {
                self.navigate_to(id, location, super::nav::NavKind::Push);
            }
        }
        self.state.focused_tile = focused;
        self.load_visible_restored_tabs();
        if result.changed {
            atlas_core::util::config::save_profiles(&self.state.profiles);
        }
        if session_changed && self.legacy_session_writer && self.state.settings.autosave_session {
            atlas_core::util::config::save_session(&self.state.pane_tree);
        }
        let mut notified = std::collections::HashSet::new();
        for (old, new) in result.moved {
            let from = old.to_string_lossy().chars().take(2).collect::<String>();
            let to = new.to_string_lossy().chars().take(2).collect::<String>();
            if notified.insert((from.clone(), to.clone())) {
                self.toast_info(rust_i18n::t!("volume_path_updated", from = from, to = to).to_string());
            }
        }
    }

    pub(crate) fn sync_split_details_layout(&mut self, cx: &mut gpui::Context<Self>) {
        let active = self.split_pane_count() > 1;
        if self.split_details_active != active {
            self.split_details_active = active;
            self.notify_all_panes(cx);
        }
    }

    pub(crate) fn details_prefs_for(&self, pane: atlas_model::pane::TabId) -> crate::app::DetailsColumnPrefs {
        if self.split_details_active {
            return self.split_details_prefs;
        }
        let fallback = crate::app::DetailsColumnPrefs { widths: self.col_widths, order: self.details_column_order };
        let Some(columns) = self.tab_data_for(pane).and_then(|tab| tab.details_columns.as_ref()) else { return fallback; };
        crate::app::DetailsColumnPrefs::from_folder(columns).unwrap_or(fallback)
    }

    pub(crate) fn set_details_prefs(&mut self, pane: atlas_model::pane::TabId, prefs: crate::app::DetailsColumnPrefs) {
        if self.split_details_active {
            self.split_details_prefs = prefs;
            return;
        }
        if let Some(atlas_model::layout_tree::Tile::Pane(tab)) = self.state.pane_tree.tiles.get_mut(pane) {
            tab.details_columns = Some(prefs.to_folder());
        }
    }

    pub(crate) fn switch_profile(&mut self, idx: usize) {
        if idx == self.state.profiles.active_idx || idx >= self.state.profiles.profiles.len() {
            return;
        }
        if !self.close_native_vault(false) { return; }
        self.file_filter_projections.clear();
        // Keys and decrypted names never cross a profile boundary.
        self.vault = None;

        // Snapshot current tree + view/layout knobs into the active profile
        // before leaving, so they restore when we come back.
        let leaving_ws = self.state.settings.workspace();
        {
            let leaving = self.state.profiles.active_mut();
            leaving.tree = Some(self.state.pane_tree.clone());
            leaving.workspace = leaving_ws;
            if leaving.prefs.contains(atlas_model::profiles::SETTINGS_PREFS_KEY) {
                leaving.prefs.set(atlas_model::profiles::SETTINGS_PREFS_KEY, &self.state.settings);
            }
        }

        // Drop all FS watchers from the current session
        self.watchers.clear();
        self.moved_watchers.clear();

        self.state.profiles.active_idx = idx;
        self.split_details_prefs = SplitDetailsColumns::for_profile(self.state.profiles.active());
        // Overlay the entering profile's saved view/layout knobs onto live settings.
        let entering_ws = self.state.profiles.active().workspace.clone();
        if let Some(mut settings) = self.state.profiles.active().prefs.get::<atlas_model::settings::Settings>(atlas_model::profiles::SETTINGS_PREFS_KEY) {
            settings.window_geometry = self.state.settings.window_geometry.clone();
            self.state.settings = settings;
        }
        self.state.settings.apply_workspace(&entering_ws);
        rust_i18n::set_locale(self.state.settings.language.as_deref().unwrap_or("en"));
        // Swap the icon-theme mappings to the entering profile's selection.
        self.apply_profile_icons();
        // Each profile carries its own theme — apply it on next render.
        self.theme_dirty = true;
        // Re-apply the entering profile's sidebar width to the left dock next render.
        self.sidebar_width_dirty = true;
        self.state.profile_ui.adding_bookmark = false;
        self.state.profile_ui.adding_tag = false;
        self.state.profile_ui.creating_tag = false;

        // Restore the target profile's tree, or create a fresh single-tab home pane
        let new_tree = self
            .state
            .profiles
            .active()
            .tree
            .clone()
            .unwrap_or_else(|| {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                let tab_data = TabData::with_view(
                    home,
                    self.state.settings.view_mode.clone(),
                    self.state.settings.thumbnail_size,
                    self.state.settings.show_inspector,
                );
                let mut tree = Tree::empty();
                let pane_id = tree.tiles.insert_pane(tab_data);
                let tabs_id = tree.tiles.insert_tab_tile(vec![pane_id]);
                tree.root = Some(tabs_id);
                tree
            });

        self.state.pane_tree = new_tree;
        super::windowing::hide_remote_locations(&mut self.state.pane_tree);
        // The Dock caches FilePanels keyed by TileId; without advancing the layout
        // revision `ensure_dock` would keep rendering the previous profile's panes
        // (now dangling tile ids -> "Empty folder" / blank). Force a center rebuild.
        self.layout_revision += 1;
        self.state.tab_states.clear();
        self.state.preview.clear_all();

        let pane_ids = self.state.all_pane_tile_ids();
        if let Some(first) = self.state.pane_tree.visible_panes().first().copied() {
            self.state.focused_tile = first;
        }

        let folder_views = atlas_core::util::config::folder_view_prefs(self.state.profiles.active());
        for tile_id in pane_ids {
            self.state.tab_states.entry(tile_id).or_default();
            if let Some(Tile::Pane(tab)) = self.state.pane_tree.tiles.get_mut(tile_id) {
                super::nav::apply_folder_view_pref(tab, &folder_views);
            }
        }
        self.restored_tabs = super::fs_sync::restore::RestoredTabs::new(&self.state, None);
        self.load_visible_restored_tabs();

        // Persist the swap immediately so a crash before window close keeps the
        // active profile and its restored view/layout.
        atlas_core::util::config::save_profiles(&self.state.profiles);
        atlas_core::util::config::save_settings(&self.state.settings);
    }

    /// Create a new profile seeded from the *current* view/layout and theme, then
    /// switch to it. The fresh profile starts with no saved tree, so the switch
    /// opens a clean home pane.
    pub(crate) fn create_profile(&mut self, name: impl Into<String>) {
        if !self.allow_edition_action(
            atlas_model::edition::Feature::Profiles,
            self.state.profiles.profiles.len().saturating_add(1),
        ) {
            return;
        }
        let name = name.into();
        let name = if name.trim().is_empty() {
            self.unique_profile_name()
        } else {
            name.trim().to_string()
        };
        let mut profile = atlas_model::profiles::Profile::new(name);
        // Inherit the current look so a new profile doesn't snap to hard defaults.
        profile.workspace = self.state.settings.workspace();
        profile.theme = self.state.profiles.active().theme.clone();
        self.state.profiles.profiles.push(profile);
        let new_idx = self.state.profiles.profiles.len() - 1;
        self.switch_profile(new_idx);
    }

    pub(crate) fn duplicate_profile(&mut self, idx: usize) {
        let Some(source) = self.state.profiles.profiles.get(idx).cloned() else {
            return;
        };
        if !self.allow_edition_action(
            atlas_model::edition::Feature::Profiles,
            self.state.profiles.profiles.len().saturating_add(1),
        ) {
            return;
        }
        let mut profile = source;
        // Copying appearance/preferences must not copy private-folder identity.
        profile.id = atlas_model::profiles::Profile::new("").id;
        profile.name = self.unique_profile_copy_name(&profile.name);
        self.state.profiles.profiles.push(profile);
        let new_idx = self.state.profiles.profiles.len() - 1;
        self.state.profile_ui.confirming_delete_profile = None;
        self.switch_profile(new_idx);
    }

    /// Build a fresh profile name like "Perfil 2" that doesn't collide with an
    /// existing one.
    fn unique_profile_name(&self) -> String {
        let taken: std::collections::HashSet<&str> = self
            .state
            .profiles
            .profiles
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        (1..)
            .map(|n| rust_i18n::t!("profile_default_name", n = n).to_string())
            .find(|name| !taken.contains(name.as_str()))
            .unwrap_or_else(|| rust_i18n::t!("profile_default_name", n = 1).to_string())
    }

    fn unique_profile_copy_name(&self, base: &str) -> String {
        let taken: std::collections::HashSet<&str> = self
            .state
            .profiles
            .profiles
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        let first = rust_i18n::t!("profile_copy_name", name = base).to_string();
        if !taken.contains(first.as_str()) {
            return first;
        }
        (2..)
            .map(|n| rust_i18n::t!("profile_copy_name_numbered", name = base, n = n).to_string())
            .find(|name| !taken.contains(name.as_str()))
            .unwrap_or_else(|| rust_i18n::t!("profile_copy_name", name = base).to_string())
    }

    /// Rename the active profile. Empty/whitespace names are ignored.
    pub(crate) fn rename_active_profile(&mut self, name: impl Into<String>) {
        let idx = self.state.profiles.active_idx;
        self.rename_profile(idx, name);
    }

    pub(crate) fn rename_profile(&mut self, idx: usize, name: impl Into<String>) {
        let name = name.into();
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }
        let Some(profile) = self.state.profiles.profiles.get_mut(idx) else {
            return;
        };
        profile.name = trimmed.to_string();
        atlas_core::util::config::save_profiles(&self.state.profiles);
    }

    pub(crate) fn delete_active_profile(&mut self) {
        let idx = self.state.profiles.active_idx;
        self.delete_profile(idx);
    }

    pub(crate) fn delete_profile(&mut self, del_idx: usize) {
        if self.state.profiles.profiles.len() <= 1 {
            return;
        }
        if del_idx >= self.state.profiles.profiles.len() {
            return;
        }

        let was_active = del_idx == self.state.profiles.active_idx;
        if was_active {
            let target = if del_idx == 0 { 1 } else { del_idx - 1 };
            self.switch_profile(target);
            // switch_profile can refuse (e.g. files open in a mounted vault);
            // never remove the profile that is still live.
            if self.state.profiles.active_idx == del_idx {
                self.state.profile_ui.confirming_delete_profile = None;
                return;
            }
        }

        self.state.profiles.profiles.remove(del_idx);
        if del_idx < self.state.profiles.active_idx {
            self.state.profiles.active_idx -= 1;
        } else if self.state.profiles.active_idx >= self.state.profiles.profiles.len() {
            self.state.profiles.active_idx = self.state.profiles.profiles.len() - 1;
        }
        if self.renaming_profile_idx.is_some_and(|idx| idx == del_idx) {
            self.renaming_profile_idx = None;
            self.rename_profile_input = None;
            self.state.overlay.renaming_profile = false;
        } else if let Some(idx) = self.renaming_profile_idx.as_mut() {
            if del_idx < *idx {
                *idx -= 1;
            }
        }
        self.state.profile_ui.confirming_delete_profile = None;
        atlas_core::util::config::save_profiles(&self.state.profiles);
        if was_active {
            atlas_core::util::config::save_settings(&self.state.settings);
        } else {
            self.layout_revision += 1;
        }
    }
}

/// Per-profile Details-view column preferences (widths + visual order).
/// Blueprint: same pattern as `IconSelection`.
pub(crate) struct DetailsColumns;
impl atlas_model::profiles::ProfileScoped for DetailsColumns {
    const KEY: &'static str = "details_columns";
    type State = crate::app::DetailsColumnPrefs;
}

pub(crate) struct SplitDetailsColumns;
impl atlas_model::profiles::ProfileScoped for SplitDetailsColumns {
    const KEY: &'static str = "split_details_columns";
    type State = Option<crate::app::DetailsColumnPrefs>;
}

impl SplitDetailsColumns {
    pub(super) fn for_profile(profile: &atlas_model::profiles::Profile) -> crate::app::DetailsColumnPrefs {
        profile.scoped::<Self>().unwrap_or_else(|| {
            let mut prefs = crate::app::DetailsColumnPrefs::default();
            for column in crate::app::DetailsColumn::DEFAULT_ORDER {
                prefs.widths.set_visible(column, column == crate::app::DetailsColumn::Size);
            }
            prefs
        })
    }
}

impl App {
    /// Snapshot the live column widths + order into the active profile and persist.
    /// Called on resize/reorder release (discrete events — no debounce needed).
    pub(crate) fn persist_details_columns(&mut self, pane: atlas_model::pane::TabId) {
        let prefs = self.details_prefs_for(pane);
        if self.split_details_active {
            self.state.profiles.active_mut().set_scoped::<SplitDetailsColumns>(&Some(prefs));
            atlas_core::util::config::save_profiles(&self.state.profiles);
            return;
        }
        self.col_widths = prefs.widths;
        self.details_column_order = prefs.order;
        self.state
            .profiles
            .active_mut()
            .set_scoped::<DetailsColumns>(&prefs);
        let folder = self.tab_data_for(pane).and_then(|tab| match &tab.location {
            atlas_model::location::Location::Directory(path) => Some((path.clone(), atlas_model::profiles::FolderViewPref {
                view_mode: tab.view_mode.clone(), sort_col: tab.sort_col.clone(), sort_dir: tab.sort_dir.clone(),
                recursive: false, columns: tab.details_columns.clone(),
            })), _ => None,
        });
        if !self.save_folder_view_pref(folder) {
            atlas_core::util::config::save_profiles(&self.state.profiles);
        }
    }
}

/// Official per-profile icon-theme selection: the `extension → SVG-path` mappings
/// that override the built-in file-type icons. Produced by the `icon-manager`
/// extension, persisted per profile through `ProfilePrefs`, and rebuilt into the
/// live `IconThemeIndex` (resvg pipeline) on every profile switch.
///
/// First concrete consumer of the `ProfileScoped` contract — the blueprint any
/// other "persist this per profile" feature should copy.
pub(crate) struct IconSelection;

impl atlas_model::profiles::ProfileScoped for IconSelection {
    const KEY: &'static str = "icons";
    type State = std::collections::HashMap<String, String>;
}

impl App {
    /// Build an [`IconThemeIndex`] from the active profile's saved icon mappings,
    /// falling back to VSCode auto-discovery when the profile has no selection.
    /// Shared by startup and live switching so both paths agree.
    pub(crate) fn icon_theme_for_active_profile(
        &self,
    ) -> Option<crate::icon_theme::IconThemeIndex> {
        let map = self.state.profiles.active().scoped::<IconSelection>();
        if map.is_empty() {
            crate::icon_theme::find_vscode_theme()
        } else {
            let paths = map
                .into_iter()
                .map(|(ext, path)| (ext, std::path::PathBuf::from(path)))
                .collect();
            crate::icon_theme::IconThemeIndex::from_mappings(paths)
        }
    }

    /// Swap the live icon theme to the active profile's selection and drop the
    /// rasterised cache so icons re-render through the resvg pipeline (the same
    /// one VSCode themes use). Callers re-list affected dirs to re-warm the cache.
    pub(crate) fn apply_profile_icons(&mut self) {
        self.icon_theme = self.icon_theme_for_active_profile();
        self.icon_cache.clear();
    }

    /// Record icon mappings just produced by an extension into the active profile,
    /// persist, and apply them live.
    pub(crate) fn persist_icon_selection(
        &mut self,
        map: std::collections::HashMap<String, String>,
    ) {
        self.state
            .profiles
            .active_mut()
            .set_scoped::<IconSelection>(&map);
        atlas_core::util::config::save_profiles(&self.state.profiles);
        // Choosing an icon theme means "show it": native Shell icons would
        // otherwise keep winning under the Windows themes.
        use atlas_model::settings::IconStyle;
        let style = if map.is_empty() { IconStyle::Windows } else { IconStyle::Theme };
        if self.state.settings.icon_style != style {
            self.state.settings.icon_style = style;
            atlas_core::util::config::save_settings(&self.state.settings);
        }
        self.apply_profile_icons();
    }

    /// Official per-profile variable store for an extension. Variables live as a
    /// JSON object under the `ext:<id>` namespace in the active profile's
    /// [`ProfilePrefs`], so each profile keeps its own extension configuration.
    ///
    /// This is the sanctioned way for an extension to persist settings that should
    /// follow the profile (the icon-manager's mappings are the worked example via
    /// [`App::persist_icon_selection`]). The WASM host-function bridge that lets a
    /// plugin call these is the remaining wiring.
    #[allow(dead_code)] // reason: official API surface; WASM bridge wires callers next.
    pub(crate) fn ext_var(&self, ext_id: &str, key: &str) -> Option<serde_json::Value> {
        self.state
            .profiles
            .active()
            .prefs
            .get::<serde_json::Map<String, serde_json::Value>>(&format!("ext:{ext_id}"))
            .and_then(|vars| vars.get(key).cloned())
    }

    #[allow(dead_code)] // reason: official API surface; WASM bridge wires callers next.
    pub(crate) fn set_ext_var(&mut self, ext_id: &str, key: &str, value: serde_json::Value) {
        let ns = format!("ext:{ext_id}");
        let mut vars = self
            .state
            .profiles
            .active()
            .prefs
            .get::<serde_json::Map<String, serde_json::Value>>(&ns)
            .unwrap_or_default();
        vars.insert(key.to_string(), value);
        self.state.profiles.active_mut().prefs.set(&ns, &vars);
        atlas_core::util::config::save_profiles(&self.state.profiles);
    }
}
