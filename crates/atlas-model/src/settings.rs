use std::path::PathBuf;

/// Normal (restore) bounds in logical desktop pixels; independent of UI zoom.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    pub monitor: Option<String>,
}

fn deserialize_window_geometry<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<WindowGeometry>, D::Error> {
    let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
    // A malformed optional placement must not discard unrelated saved preferences.
    Ok(serde_json::from_value(value).ok())
}
const fn default_thumbnail_cache_mb() -> usize { 256 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum CharacterKeyAction {
    #[default]
    TypeInFilter,
    SelectTypedItem,
    TriggerHotkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum InformationLevel { Off, #[default] Standard, Extended }
impl InformationLevel {
    pub fn index(self) -> usize { match self { Self::Off => 0, Self::Standard => 1, Self::Extended => 2 } }
    pub fn from_index(index: usize) -> Self { match index { 0 => Self::Off, 2 => Self::Extended, _ => Self::Standard } }
}

/// File icon source. Legacy Auto is equivalent to Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum IconStyle {
    Auto,
    /// Native Windows Shell icons (colored, per file type) in every theme.
    #[default]
    Windows,
    /// The installed custom icon theme.
    Theme,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ViewMode {
    #[default]
    Details,
    Grid,
    List,
    Miller,
    Tree,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum InspectorPlacement {
    #[default]
    Global,
    ActiveTab,
}

/// The subset of view/layout knobs that are saved *per profile* (sizes, view
/// mode, panel visibility, zoom). [`Settings`] holds the live working copy the UI
/// reads; switching profiles swaps this block in and out via
/// [`Settings::workspace`] / [`Settings::apply_workspace`], so call sites keep
/// reading `settings.*` unchanged.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub file_selection_opacity: f32,
    pub shortcut_target_opacity: f32,
    pub show_hidden: bool,
    pub show_sidebar: bool,
    pub sidebar_collapsed: bool,
    pub show_inspector: bool,
    pub inspector_placement: InspectorPlacement,
    pub view_mode: ViewMode,
    pub sidebar_width: f32,
    pub inspector_width: f32,
    pub font_size: f32,
    pub font_family: String,
    pub thumbnail_size: f32,
    pub global_zoom: f32,
    #[serde(default = "default_grid_spacing")]
    pub grid_spacing: f32,
    pub vim_mode: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            file_selection_opacity: 0.30,
            shortcut_target_opacity: 0.25,
            show_hidden: false,
            show_sidebar: true,
            sidebar_collapsed: false,
            show_inspector: false,
            inspector_placement: InspectorPlacement::Global,
            view_mode: ViewMode::Details,
            sidebar_width: 220.0,
            inspector_width: 560.0,
            font_size: 13.0,
            font_family: "Geist".to_string(),
            thumbnail_size: 128.0,
            global_zoom: 1.0,
            grid_spacing: default_grid_spacing(),
            vim_mode: false,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub photo_information: InformationLevel,
    pub media_information: InformationLevel,
    pub other_information: InformationLevel,
    pub file_selection_opacity: f32,
    pub shortcut_target_opacity: f32,
    pub automatic_folder_statistics: bool,
    pub folder_statistics_ssd: bool,
    pub folder_statistics_hdd: bool,
    pub folder_statistics_network: bool,
    /// Adds vertical breathing room only to Details view.
    #[serde(default)]
    pub comfortable_details_rows: bool,
    /// Display extensions in file-view labels; actual filenames are never changed.
    #[serde(default)]
    pub show_known_file_extensions: bool,
    pub detail_column_separators: bool,
    pub detail_separator_strength: f32,
    pub alternating_row_colors: bool,
    pub alternating_row_strength: f32,
    #[serde(default, deserialize_with = "deserialize_window_geometry")]
    pub window_geometry: Option<WindowGeometry>,
    pub drop_zone_expanded: bool,
    pub sidebar_places_height: Option<f32>,
    pub sidebar_drives_height: Option<f32>,
    pub toolbar_labels: bool,
    pub toolbar_order: Vec<String>,
    #[serde(default)]
    pub toolbar_new_actions_initialized: bool,
    #[serde(default)]
    pub toolbar_split_initialized: bool,
    pub single_click_folders_only: bool,
    pub scroll_overscroll: f32,
    pub character_key_action: CharacterKeyAction,
    pub filter_sticky: bool,
    #[serde(default = "default_thumbnail_cache_mb")]
    pub thumbnail_cache_mb: usize,
    pub show_hidden: bool,
    pub show_sidebar: bool,
    pub sidebar_collapsed: bool,
    pub show_inspector: bool,
    pub inspector_placement: InspectorPlacement,
    pub view_mode: ViewMode,
    pub sidebar_width: f32,
    pub inspector_width: f32,
    pub font_size: f32,
    pub font_family: String,
    pub thumbnail_size: f32,
    /// Legacy global recent folders. Migrated into per-profile prefs at startup.
    pub recent_folders: Vec<PathBuf>,
    pub global_zoom: f32,
    pub language: Option<String>,
    /// When true, all with_animation calls are bypassed: elements render at their
    /// final state immediately. Eliminates the per-frame repaint cost of running
    /// animations across multiple panes.
    pub disable_animations: bool,
    /// Mouse-wheel scroll smoothness, 0.0 (instant, no easing) … 1.0 (long silky
    /// glide). Controls animation timing only. Global, like
    /// `disable_animations`. Trackpad (pixel-precise) input is unaffected.
    pub scroll_smoothness: f32,
    /// Wheel distance multiplier. Precision pixel input is unchanged.
    pub scroll_speed: f32,
    /// Show Atlas' custom top window bar. Turn off when the platform/window manager
    /// already provides chrome and the extra in-app strip is unwanted.
    pub show_titlebar: bool,
    /// When true, evicts thumbnails and shrinks collections when the window is inactive.
    pub eco_mode: bool,
    /// When true, the pane/tab layout is persisted to `session.json` when the
    /// window closes, so the next launch restores it. Off = the last explicit
    /// save (theme change, manual "Save Session" command) still applies.
    pub autosave_session: bool,
    /// Visually de-emphasize panes other than the focused pane.
    pub dim_inactive_panes: bool,
    /// Background opacity for Glass themes (0.20 to 1.00, default 0.80).
    #[serde(default = "default_glass_opacity")]
    pub glass_opacity: f32,
    /// Element spacing / gap in grid view (points, default 16.0).
    #[serde(default = "default_grid_spacing")]
    pub grid_spacing: f32,
    /// True after the first-run setup has been dismissed or completed.
    pub getting_started_seen: bool,
    pub vim_mode: bool,
    pub icon_style: IconStyle,
    /// Convert / Actions radial menu preferences.
    pub ring: RingPrefs,
}

/// Convert / Actions ring preferences. Unknown action ids (from a newer or
/// older build) are ignored by the UI, so a stale list never breaks loading.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RingPrefs {
    /// Open on the Actions side instead of Convert.
    pub open_actions: bool,
    /// Visible actions, in ring order (ids from `convert::actions::Action::id`).
    pub actions: Vec<String>,
    /// Longest edge for "Resize", in pixels.
    pub resize_edge: u32,
    /// JPEG quality for "Compress".
    pub compress_quality: u8,
}

pub const RING_ACTION_IDS: [&str; 6] = [
    "resize",
    "rotate-right",
    "rotate-left",
    "strip-metadata",
    "compress",
    "compress-pdf",
];

impl Default for RingPrefs {
    fn default() -> Self {
        Self {
            open_actions: false,
            actions: RING_ACTION_IDS.iter().map(|id| (*id).to_string()).collect(),
            resize_edge: 1920,
            compress_quality: 80,
        }
    }
}

impl RingPrefs {
    pub fn shows(&self, id: &str) -> bool {
        self.actions.iter().any(|action| action == id)
    }
    /// Show/hide `id`, keeping the canonical order for re-added actions.
    pub fn set_shown(&mut self, id: &str, shown: bool) {
        self.actions.retain(|action| action != id);
        if shown {
            let rank = |a: &str| RING_ACTION_IDS.iter().position(|x| *x == a).unwrap_or(usize::MAX);
            let at = self
                .actions
                .iter()
                .position(|action| rank(action) > rank(id))
                .unwrap_or(self.actions.len());
            self.actions.insert(at, id.to_string());
        }
    }
}

pub fn default_glass_opacity() -> f32 {
    0.80
}

pub fn default_grid_spacing() -> f32 {
    16.0
}

/// Absolute selected-file fill alpha, independent of the theme accent's alpha.
pub fn selection_alpha(value: f32) -> f32 {
    if value.is_finite() { value.clamp(0.10, 0.70) } else { 0.30 }
}

/// Integer percent arithmetic for the 1% appearance controls. Existing stored
/// fractions are preserved until the user changes a slider.
pub fn percent_step(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() { (value.clamp(min, max) * 100.0).round() / 100.0 } else { min }
}

pub fn shortcut_alpha(value: f32) -> f32 {
    if value.is_finite() { percent_step(value, 0.0, 0.60) } else { 0.25 }
}

impl Settings {
    /// Reset preferences only; user history, onboarding and window placement are data.
    pub fn reset_preferences(&mut self) {
        let previous = self.clone();
        *self = Self::default();
        self.recent_folders = previous.recent_folders;
        self.getting_started_seen = previous.getting_started_seen;
        self.window_geometry = previous.window_geometry;
        self.migrate_split_toolbar_slot();
    }

    pub fn file_selection_alpha(&self) -> f32 { selection_alpha(self.file_selection_opacity) }

    /// Split View now has a permanent home beside View Mode. Remove only its
    /// retired toolbar slot, preserving every other customized action/order.
    pub fn migrate_split_toolbar_slot(&mut self) {
        self.toolbar_order.retain(|id| id != "split");
        self.toolbar_split_initialized = true;
    }
    pub fn scroll_overscroll_fraction(&self) -> f32 {
        if self.scroll_overscroll.is_finite() { self.scroll_overscroll.clamp(0.0, 0.9) } else { 0.4 }
    }
    pub fn single_click_opens_folder(&self, is_dir: bool, clicks: usize, modified: bool) -> bool {
        self.single_click_folders_only && is_dir && clicks == 1 && !modified
    }
    /// Snapshot the per-profile view/layout knobs out of the live settings.
    pub fn workspace(&self) -> Workspace {
        Workspace {
            file_selection_opacity: self.file_selection_alpha(),
            shortcut_target_opacity: shortcut_alpha(self.shortcut_target_opacity),
            show_hidden: self.show_hidden,
            show_sidebar: self.show_sidebar,
            sidebar_collapsed: self.sidebar_collapsed,
            show_inspector: self.show_inspector,
            inspector_placement: self.inspector_placement.clone(),
            view_mode: self.view_mode.clone(),
            sidebar_width: self.sidebar_width,
            inspector_width: self.inspector_width,
            font_size: self.font_size,
            font_family: self.font_family.clone(),
            thumbnail_size: self.thumbnail_size,
            global_zoom: self.global_zoom,
            grid_spacing: self.grid_spacing,
            vim_mode: self.vim_mode,
        }
    }

    /// Overlay a profile's saved knobs onto the live settings. Truly global
    /// fields (`language`, `disable_animations`) are left intact.
    pub fn apply_workspace(&mut self, ws: &Workspace) {
        self.file_selection_opacity = selection_alpha(ws.file_selection_opacity);
        self.shortcut_target_opacity = shortcut_alpha(ws.shortcut_target_opacity);
        self.show_hidden = ws.show_hidden;
        self.show_sidebar = ws.show_sidebar;
        self.sidebar_collapsed = ws.sidebar_collapsed;
        self.show_inspector = ws.show_inspector;
        self.inspector_placement = ws.inspector_placement.clone();
        self.view_mode = ws.view_mode.clone();
        self.sidebar_width = ws.sidebar_width;
        self.inspector_width = ws.inspector_width;
        self.font_size = ws.font_size;
        self.font_family.clone_from(&ws.font_family);
        self.thumbnail_size = ws.thumbnail_size;
        self.global_zoom = ws.global_zoom;
        self.grid_spacing = ws.grid_spacing;
        self.vim_mode = ws.vim_mode;
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            file_selection_opacity: 0.30,
            shortcut_target_opacity: 0.25,
            photo_information: InformationLevel::Standard,
            media_information: InformationLevel::Standard,
            other_information: InformationLevel::Standard,
            automatic_folder_statistics: true,
            folder_statistics_ssd: true,
            folder_statistics_hdd: false,
            folder_statistics_network: false,
            comfortable_details_rows: false,
            show_known_file_extensions: false,
            detail_column_separators: false,
            detail_separator_strength: 0.15,
            alternating_row_colors: false,
            alternating_row_strength: 0.3,
            window_geometry: None,
            drop_zone_expanded: false,
            sidebar_places_height: None,
            sidebar_drives_height: None,
            toolbar_labels: false,
            toolbar_order: ["copy", "cut", "paste", "rename", "delete", "new-folder", "new-text-file", "properties", "terminal", "size", "branch"]
                .into_iter().map(str::to_owned).collect(),
            toolbar_new_actions_initialized: true,
            toolbar_split_initialized: true,
            single_click_folders_only: false,
            scroll_overscroll: 0.4,
            character_key_action: CharacterKeyAction::TypeInFilter,
            icon_style: IconStyle::Windows,
            filter_sticky: false,
            thumbnail_cache_mb: default_thumbnail_cache_mb(),
            show_hidden: false,
            show_sidebar: true,
            sidebar_collapsed: false,
            show_inspector: false,
            inspector_placement: InspectorPlacement::Global,
            view_mode: ViewMode::Details,
            sidebar_width: 220.0,
            inspector_width: 560.0,
            font_size: 13.0,
            font_family: "Geist".to_string(),
            thumbnail_size: 128.0,
            recent_folders: Vec::new(),
            global_zoom: 1.0,
            language: None,
            disable_animations: false,
            scroll_smoothness: 0.6,
            scroll_speed: 1.0,
            show_titlebar: true,
            eco_mode: true,
            autosave_session: true,
            dim_inactive_panes: false,
            glass_opacity: default_glass_opacity(),
            grid_spacing: default_grid_spacing(),
            getting_started_seen: false,
            vim_mode: false,
            ring: RingPrefs::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comfortable_details_rows_default_and_persist() {
        let old: Settings = serde_json::from_str(r#"{"font_size":13.0}"#).unwrap();
        assert!(!old.comfortable_details_rows);
        let mut settings = old;
        settings.comfortable_details_rows = true;
        let reloaded: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(reloaded.comfortable_details_rows);
        assert_eq!(reloaded.font_size, 13.0);
    }

    #[test]
    fn file_selection_opacity_defaults_clamps_and_keeps_other_settings() {
        let old: Settings = serde_json::from_str(r#"{"font_family":"Custom Font","global_zoom":1.25}"#).unwrap();
        assert_eq!(old.file_selection_alpha(), 0.30);
        assert_eq!(selection_alpha(0.0), 0.10);
        assert_eq!(selection_alpha(1.0), 0.70);
        assert_eq!(selection_alpha(f32::NAN), 0.30);
        for value in [0.10, 0.30, 0.70] { assert_eq!(selection_alpha(value), value); }
        assert_eq!(old.font_family, "Custom Font");
        assert_eq!(old.global_zoom, 1.25);
    }

    #[test]
    fn file_selection_opacity_round_trips_through_profile_workspace() {
        let mut profiles = crate::profiles::ProfileManager::default();
        let mut settings = Settings::default();
        settings.file_selection_opacity = 0.55;
        profiles.active_mut().workspace = settings.workspace();
        let profiles: crate::profiles::ProfileManager = serde_json::from_slice(&serde_json::to_vec(&profiles).unwrap()).unwrap();
        settings.file_selection_opacity = 0.10;
        settings.apply_workspace(&profiles.active().workspace);
        assert_eq!(settings.file_selection_alpha(), 0.55);
        assert_eq!(serde_json::from_str::<Workspace>("{}").unwrap().file_selection_opacity, 0.30);
        let restored: Settings = serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(restored.file_selection_alpha(), 0.55);
    }
    #[test]
    fn relocated_split_slot_preserves_customization_and_is_idempotent() {
        let mut settings: Settings = serde_json::from_str(r#"{"toolbar_order":["terminal","split","copy","unknown-extension","branch"],"toolbar_labels":true,"font_family":"Custom Font","folder_statistics_hdd":true}"#).unwrap();
        settings.migrate_split_toolbar_slot();
        settings.migrate_split_toolbar_slot();
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.toolbar_order, ["terminal", "copy", "unknown-extension", "branch"]);
        assert!(restored.toolbar_split_initialized && restored.toolbar_labels && restored.folder_statistics_hdd);
        assert_eq!(restored.font_family, "Custom Font");
        assert!(!Settings::default().toolbar_order.iter().any(|id| id == "split"));
    }
    #[test]
    fn toolbar_defaults_and_old_custom_visibility_survive_serialization() {
        let defaults = Settings::default();
        assert_eq!(defaults.toolbar_order, ["copy", "cut", "paste", "rename", "delete", "new-folder", "new-text-file", "properties", "terminal", "size", "branch"]);
        let old: Settings = serde_json::from_str(r#"{"toolbar_order":["terminal","copy","branch"],"toolbar_labels":true,"toolbar_new_actions_initialized":true,"font_family":"Custom Font"}"#).unwrap();
        assert!(!old.toolbar_split_initialized);
        assert_eq!(old.toolbar_order, ["terminal", "copy", "branch"]);
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
        assert_eq!(restored.toolbar_order, old.toolbar_order);
        assert!(restored.toolbar_labels);
        assert_eq!(restored.font_family, "Custom Font");
        assert!(!restored.toolbar_split_initialized);
    }
    #[test]
    fn group2_settings_are_backward_compatible_and_round_trip() {
        let old: Settings = serde_json::from_str(r#"{"sidebar_width":284,"font_family":"Segoe UI","grid_spacing":22}"#).unwrap();
        assert!(!old.detail_column_separators);
        assert_eq!(old.detail_separator_strength, 0.15);
        assert!(!old.alternating_row_colors);
        assert!(!old.drop_zone_expanded);
        assert!(old.window_geometry.is_none());
        let mut settings = old;
        settings.detail_column_separators = true;
        settings.detail_separator_strength = 0.45;
        settings.alternating_row_colors = true;
        settings.alternating_row_strength = 0.65;
        settings.drop_zone_expanded = true;
        settings.window_geometry = Some(WindowGeometry {
            x: -1600.0, y: 80.0, width: 1200.0, height: 760.0,
            maximized: true, monitor: Some("saved-monitor".into()),
        });
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.window_geometry, settings.window_geometry);
        assert!(restored.detail_column_separators);
        assert_eq!(restored.detail_separator_strength, 0.45);
        assert!(restored.alternating_row_colors && restored.drop_zone_expanded);
        assert_eq!(restored.alternating_row_strength, 0.65);
        assert_eq!(restored.workspace(), settings.workspace());
        assert_eq!(restored.sidebar_width, 284.0);
        assert_eq!(restored.grid_spacing, 22.0);
        assert_eq!(restored.font_family, "Segoe UI");
    }

    #[test]
    fn invalid_window_geometry_does_not_reset_other_preferences() {
        for geometry in [r#"{"width": "bad"}"#, r#"{"width": 100}"#, "null"] {
            let json = format!(r#"{{"sidebar_width":310,"grid_spacing":18,"window_geometry":{geometry}}}"#);
            let settings: Settings = serde_json::from_str(&json).unwrap();
            assert!(settings.window_geometry.is_none());
            assert_eq!(settings.sidebar_width, 310.0);
            assert_eq!(settings.grid_spacing, 18.0);
        }
    }
    #[test]
    fn scroll_margin_defaults_to_forty_percent_and_rejects_invalid_extents() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.scroll_overscroll_fraction(), 0.4);
        for (value, expected) in [(-1.0, 0.0), (0.0, 0.0), (0.4, 0.4), (5.0, 0.9), (f32::NAN, 0.4)] {
            settings.scroll_overscroll = value;
            assert_eq!(settings.scroll_overscroll_fraction(), expected);
        }
    }

    #[test]
    fn single_click_never_launches_files_or_modified_folder_clicks() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(!settings.single_click_opens_folder(true, 1, false));
        settings.single_click_folders_only = true;
        assert!(settings.single_click_opens_folder(true, 1, false));
        assert!(!settings.single_click_opens_folder(false, 1, false));
        assert!(!settings.single_click_opens_folder(true, 1, true));
        assert!(!settings.single_click_opens_folder(true, 2, false));
        let saved = serde_json::to_string(&settings).unwrap();
        assert!(serde_json::from_str::<Settings>(&saved).unwrap().single_click_folders_only);
    }

    #[test]
    fn ring_prefs_load_from_old_and_partial_settings() {
        let old: Settings = serde_json::from_str(r#"{"font_size": 13.0}"#).unwrap();
        assert_eq!(old.ring, RingPrefs::default());
        let partial: Settings =
            serde_json::from_str(r#"{"ring": {"actions": ["compress", "future-action"]}}"#).unwrap();
        assert_eq!(partial.ring.actions, ["compress", "future-action"]);
        assert_eq!(partial.ring.resize_edge, 1920);
        let mut ring = partial.ring;
        ring.set_shown("resize", true);
        ring.set_shown("compress", false);
        assert!(ring.shows("resize") && !ring.shows("compress"));
        assert_eq!(ring.actions[0], "resize");
    }

    #[test]
    fn settings_defaults_glass_opacity_when_absent() {
        let json = r#"{"font_size": 13.0}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!((s.glass_opacity - 0.80).abs() < 1e-4);
    }

    #[test]
    fn settings_preserves_custom_glass_opacity() {
        let json = r#"{"glass_opacity": 0.55}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!((s.glass_opacity - 0.55).abs() < 1e-4);
    }

    #[test]
    fn settings_defaults_grid_spacing_when_absent() {
        let json = r#"{"font_size": 13.0}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!((s.grid_spacing - 16.0).abs() < 1e-4);
    }

    #[test]
    fn settings_preserves_custom_grid_spacing() {
        let json = r#"{"grid_spacing": 24.0}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!((s.grid_spacing - 24.0).abs() < 1e-4);
    }

    #[test]
    fn settings_defaults_vim_mode_to_false() {
        let json = r#"{"font_size": 13.0}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(!s.vim_mode);

        let ws: Workspace = serde_json::from_str(json).unwrap();
        assert!(!ws.vim_mode);
    }

    #[test]
    fn settings_preserves_vim_mode() {
        let json = r#"{"vim_mode": true}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(s.vim_mode);

        let ws: Workspace = serde_json::from_str(json).unwrap();
        assert!(ws.vim_mode);
    }

    #[test]
    fn workspace_roundtrip_vim_mode() {
        let mut s = Settings::default();
        assert!(!s.vim_mode);
        s.vim_mode = true;
        let ws = s.workspace();
        assert!(ws.vim_mode);

        let mut s2 = Settings::default();
        s2.apply_workspace(&ws);
        assert!(s2.vim_mode);
    }
}

