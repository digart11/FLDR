//! Settings palette: every preference is declared once in [`SETTINGS`] —
//! category, icon, i18n title/description, search keywords and a typed
//! [`Control`] (switch, slider, segmented, searchable choice, action, page).
//!
//! `render.rs` turns the table into a command-palette–style surface: fuzzy
//! search, category pills, keyboard model (↑↓ move, ←→ adjust, Enter change,
//! Tab category, Esc back) and drill-in pickers with live preview. Every
//! mutation funnels through [`App::commit_settings`], so persistence and the
//! per-profile workspace snapshot can no longer drift per row.

mod render;

use crate::app::App;
use crate::input::action::Action;
use crate::ui::icons::AtlasIcon;
use atlas_model::settings::{CharacterKeyAction, IconStyle, InspectorPlacement, Settings, ViewMode};
use gpui::{Hsla, SharedString};
use std::sync::LazyLock;

/// Top-level grouping, shown as pills under the search row and as section
/// headers in the unfiltered list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Category {
    General,
    Appearance,
    Files,
    Layout,
    Performance,
    FolderStatistics,
    About,
}

impl Category {
    pub(crate) const ALL: [Self; 7] = [
        Self::General,
        Self::Appearance,
        Self::Files,
        Self::Layout,
        Self::Performance,
        Self::FolderStatistics,
        Self::About,
    ];

    pub(crate) fn title(self) -> SharedString {
        rust_i18n::t!(match self {
            Self::General => "sp_cat_general",
            Self::Appearance => "sp_cat_appearance",
            Self::Files => "sp_cat_files",
            Self::Layout => "sp_cat_layout",
            Self::Performance => "sp_cat_performance",
            Self::FolderStatistics => "sp_cat_folder_statistics",
            Self::About => "sp_cat_about",
        })
        .to_string()
        .into()
    }

    pub(crate) const fn icon(self) -> AtlasIcon {
        match self {
            Self::General => AtlasIcon::Gear,
            Self::Appearance => AtlasIcon::Palette,
            Self::Files => AtlasIcon::Folder,
            Self::Layout => AtlasIcon::AppWindow,
            Self::Performance => AtlasIcon::Gauge,
            Self::FolderStatistics => AtlasIcon::FolderMeasure,
            Self::About => AtlasIcon::Info,
        }
    }
}

/// One entry of a drill-in picker.
pub(crate) struct ChoiceOption {
    pub id: SharedString,
    pub label: SharedString,
    /// Muted text after the label ("Recommended", "Glass").
    pub detail: Option<SharedString>,
    /// Render the label in this font family (font picker).
    pub font: Option<SharedString>,
    /// Background / accent / foreground dots (theme picker).
    pub swatch: Option<[Hsla; 3]>,
}

impl ChoiceOption {
    fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            font: None,
            swatch: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Unit {
    Px,
    /// Stored as a 0..=1 fraction, shown ×100.
    Percent,
    Mb,
    Order,
}

pub(crate) struct Slider {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub unit: Unit,
    pub get: fn(&App) -> f32,
    pub set: fn(&mut App, f32),
    pub default: Option<fn(&Settings) -> f32>,
    /// `false`: dragging shows the pending value and applies it on release
    /// (UI zoom would otherwise rescale the track under the cursor).
    pub live: bool,
}

impl Slider {
    pub(crate) fn snap(&self, value: f32) -> f32 {
        let steps = ((value - self.min) / self.step).round();
        let snapped = (self.min + steps * self.step).clamp(self.min, self.max);
        if matches!(self.unit, Unit::Percent) && self.step == 0.01 {
            atlas_model::settings::percent_step(snapped, self.min, self.max)
        } else { snapped }
    }

    pub(crate) fn fraction(&self, value: f32) -> f32 {
        ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    pub(crate) fn format(&self, value: f32) -> String {
        match self.unit {
            Unit::Px => format!("{value:.0} px"),
            Unit::Percent => format!("{:.0}%", value * 100.0),
            Unit::Mb => format!("{value:.0} MB"),
            Unit::Order => if value < 0.5 { rust_i18n::t!("toolbar_hidden").into_owned() }
                else { rust_i18n::t!("toolbar_position", position = format!("{value:.0}")).into_owned() },
        }
    }
}

pub(crate) enum Control {
    Toggle {
        get: fn(&App) -> bool,
        set: fn(&mut App, bool),
        default: Option<fn(&Settings) -> bool>,
    },
    Slider(Slider),
    /// Two to five short options rendered inline (optionally with icons).
    Segmented {
        labels: fn() -> Vec<SharedString>,
        icons: &'static [AtlasIcon],
        get: fn(&App) -> usize,
        set: fn(&mut App, usize),
        default: Option<fn(&Settings) -> usize>,
    },
    /// Searchable drill-in list. `preview(Some(id))` shows an option live,
    /// `preview(None)` reverts; `pick` commits (and clears its preview state).
    Choice {
        options: fn(&mut App, &gpui::App) -> Vec<ChoiceOption>,
        current: fn(&App) -> SharedString,
        pick: fn(&mut App, &str),
        preview: Option<fn(&mut App, Option<&str>)>,
        default: Option<fn(&Settings) -> SharedString>,
    },
    Action {
        run: fn(&mut App),
        external: bool,
    },
    Info {
        value: fn(&App) -> SharedString,
    },
    /// Drill into a full custom page (theme editor, shortcut editor).
    /// `searchable` pages keep the search row live and filter their list.
    Page { searchable: bool },
}

pub(crate) struct SettingDef {
    pub id: &'static str,
    pub category: Category,
    pub icon: AtlasIcon,
    /// i18n key.
    pub title: &'static str,
    /// i18n key.
    pub desc: Option<&'static str>,
    /// Extra multilingual search terms, space separated.
    pub keywords: &'static str,
    pub control: Control,
}

impl SettingDef {
    pub(crate) fn title(&self) -> SharedString {
        // Localized label can replace this temporary English text once locale
        // resources are included in a later focused translation patch.
        if self.id == "details-row-spacing" {
            return "Details view spacing".into();
        }
        rust_i18n::t!(self.title).to_string().into()
    }

    pub(crate) fn desc(&self) -> Option<SharedString> {
        self.desc.map(|key| rust_i18n::t!(key).to_string().into())
    }

    /// Literal match for the secondary fields: every query word must occur in
    /// the title, keywords, description or category. Fuzzy matching is kept to
    /// titles only — scattered subsequences over long keyword strings matched
    /// nearly everything.
    pub(crate) fn words_match(&self, query: &str) -> bool {
        let haystack = format!(
            "{} {} {} {}",
            self.title(),
            self.keywords,
            self.desc().unwrap_or_default(),
            self.category.title()
        )
        .to_lowercase();
        let mut words = query.split_whitespace().peekable();
        words.peek().is_some() && words.all(|word| haystack.contains(&word.to_lowercase()))
    }

    /// Whether the live value differs from a fresh install (shows the dot and
    /// enables reset). Settings without a meaningful default never report.
    pub(crate) fn is_modified(&self, app: &App, defaults: &Settings) -> bool {
        match &self.control {
            Control::Toggle { get, default: Some(d), .. } => get(app) != d(defaults),
            Control::Slider(s) => s
                .default
                .is_some_and(|d| (s.get)(app) - d(defaults) > s.step * 0.5 || d(defaults) - (s.get)(app) > s.step * 0.5),
            Control::Segmented { get, default: Some(d), .. } => get(app) != d(defaults),
            Control::Choice { current, default: Some(d), .. } => current(app) != d(defaults),
            _ => false,
        }
    }

    /// Restore the fresh-install value; `false` when there is none.
    pub(crate) fn reset(&self, app: &mut App) -> bool {
        let defaults = Settings::default();
        match &self.control {
            Control::Toggle { set, default: Some(d), .. } => set(app, d(&defaults)),
            Control::Slider(s) => match s.default {
                Some(d) => (s.set)(app, d(&defaults)),
                None => return false,
            },
            Control::Segmented { set, default: Some(d), .. } => set(app, d(&defaults)),
            Control::Choice { pick, default: Some(d), .. } => pick(app, &d(&defaults)),
            _ => return false,
        }
        true
    }
}

/// Short current-value text ("On", "45%", "Copper Dusk") for list badges.
pub(crate) fn value_label(def: &SettingDef, app: &mut App, cx: &gpui::App) -> Option<SharedString> {
    match &def.control {
        Control::Toggle { get, .. } => Some(t(if get(app) { "sp_on" } else { "sp_off" })),
        Control::Slider(slider) => Some(slider.format((slider.get)(app)).into()),
        Control::Segmented { labels, get, .. } => labels().get(get(app)).cloned(),
        Control::Choice { options, current, .. } => {
            let current = current(app);
            if current.is_empty() {
                return None;
            }
            Some(options(app, cx).into_iter().find(|o| o.id == current).map_or(current, |o| o.label))
        }
        Control::Info { value } => Some(value(app)),
        Control::Action { .. } | Control::Page { .. } => None,
    }
}

pub(crate) fn find(id: &str) -> Option<&'static SettingDef> {
    SETTINGS.iter().find(|def| def.id == id)
}

/// Transient UI state of the open settings palette (reset on every open).
#[derive(Default)]
pub(crate) struct SettingsUi {
    pub category_scroll: gpui::ScrollHandle,
    pub reveal_category: bool,
    pub category_width: f32,
    /// Index into the visible setting rows (headers excluded).
    pub selected: usize,
    pub confirming_reset: Option<String>,
    pub category: Option<Category>,
    pub drill: Option<Drill>,
    /// Slider being dragged and, for non-live sliders, its pending value.
    pub drag: Option<(&'static str, Option<f32>)>,
    pub slider_capture: Option<crate::ui::pointer_state::SliderCapture>,
    /// Switch that just flipped; only it animates its knob.
    pub animate: Option<&'static str>,
    pub scroll_to_selected: bool,
    pub last_query: String,
    /// Select (and scroll to) this setting on the next render.
    pub focus_id: Option<&'static str>,
    /// Setting ids of the rows rendered this frame, in order (keyboard target).
    pub visible: Vec<&'static str>,
    /// Virtualised picker list (font families can number in the hundreds).
    pub options_scroll: gpui::UniformListScrollHandle,
    /// Live-preview restore value for previews without their own slot
    /// (language, font family).
    pub preview_before: Option<String>,
    /// Window bounds of each choice chip as last painted — the dropdown
    /// anchors under its chip.
    /// Main query to put back into the search field on the next render
    /// (leaving a searchable page).
    pub restore_query: Option<String>,
    pub choice_anchors: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<&'static str, gpui::Bounds<gpui::Pixels>>>>,
}

pub(crate) struct Drill {
    pub setting: &'static str,
    /// Dropdown's own search field (choices only; pages keep the main input).
    pub input: Option<gpui::Entity<gpui_component::input::InputState>>,
    pub _input_sub: Option<gpui::Subscription>,
    /// Main-list query, restored when leaving a searchable page.
    pub outer_query: Option<String>,
    pub selected: usize,
    pub last_query: String,
    /// Option currently shown live, so previews only re-run on change.
    pub previewed: Option<SharedString>,
    /// Option ids rendered this frame, in order.
    pub visible: Vec<SharedString>,
}

/// UI languages with a locale file, shown in their own name.
pub(crate) const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("es", "Español"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("pt-BR", "Português (Brasil)"),
    ("nl", "Nederlands"),
    ("pl", "Polski"),
    ("sv", "Svenska"),
    ("tr", "Türkçe"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("vi", "Tiếng Việt"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("zh-CN", "简体中文"),
    ("zh-TW", "繁體中文"),
];

const VIEW_MODES: [(ViewMode, &str, AtlasIcon); 5] = [
    (ViewMode::Details, "settings_view_details_btn", AtlasIcon::Details),
    (ViewMode::Grid, "settings_view_grid_btn", AtlasIcon::Grid),
    (ViewMode::List, "settings_view_list_btn", AtlasIcon::List),
    (ViewMode::Miller, "settings_view_miller_btn", AtlasIcon::Columns),
    (ViewMode::Tree, "settings_view_tree_btn", AtlasIcon::TreeStructure),
];

const VIEW_ICONS: [AtlasIcon; 5] = [
    AtlasIcon::Details,
    AtlasIcon::Grid,
    AtlasIcon::List,
    AtlasIcon::Columns,
    AtlasIcon::TreeStructure,
];

const ICON_STYLES: [(IconStyle, &str, &str); 2] = [
    (IconStyle::Windows, "windows", "settings_icon_style_windows"),
    (IconStyle::Theme, "theme", "settings_icon_style_theme"),
];

fn icon_style_id(style: IconStyle) -> &'static str {
    ICON_STYLES.iter().find(|(s, ..)| *s == style).map_or("windows", |(_, id, _)| id)
}

fn icon_style_from_id(id: &str) -> Option<IconStyle> {
    ICON_STYLES.iter().find(|(_, sid, _)| *sid == id).map(|(s, ..)| *s)
}

/// Downloadable icon themes: (`icon-manager` command, label, install-dir slug).
const ICON_THEME_SOURCES: [(&str, &str, &str); 3] = [
    ("icon-manager.download-material", "Material", "PKief_material-icon-theme"),
    ("icon-manager.download-catppuccin", "Catppuccin", "catppuccin_catppuccin-vsc-icons"),
    ("icon-manager.download-flow", "Flow", "thang-nm_flow-icons"),
];

const VARIANT_PREFIX: &str = "variant:";

fn icon_data_dir() -> std::path::PathBuf {
    atlas_core::util::config::data_dir().join("extensions").join("icon-manager")
}

/// Installed icon-theme variants + the active one, scanned from disk when the
/// icon picker opens or a download finishes (the picker reads it every frame).
#[derive(Default, Clone)]
pub(crate) struct IconInventory {
    loaded: bool,
    variants: Vec<atlas_core::ext::runner::InstalledIconVariant>,
    active: Option<(String, String)>,
}

static ICON_INVENTORY: LazyLock<std::sync::Mutex<IconInventory>> = LazyLock::new(Default::default);

pub(crate) fn refresh_icon_inventory() {
    let dir = icon_data_dir();
    let inventory = IconInventory {
        loaded: true,
        variants: atlas_core::ext::runner::installed_icon_variants(&dir),
        active: atlas_core::ext::runner::active_icon_variant(&dir),
    };
    *ICON_INVENTORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = inventory;
}

fn icon_inventory() -> IconInventory {
    let inventory = ICON_INVENTORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    if inventory.loaded {
        return inventory;
    }
    refresh_icon_inventory();
    ICON_INVENTORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

fn variant_id(variant: &atlas_core::ext::runner::InstalledIconVariant) -> String {
    format!("{VARIANT_PREFIX}{}|{}", variant.slug, variant.theme_rel_path)
}

/// "Catppuccin · Mocha", or just the theme name when it has one variant.
fn variant_label(variant: &atlas_core::ext::runner::InstalledIconVariant) -> String {
    let name = ICON_THEME_SOURCES
        .iter()
        .find(|(_, _, slug)| *slug == variant.slug)
        .map_or(variant.pkg_name.as_str(), |(_, label, _)| label);
    if variant.siblings > 1 {
        // "Flow Dim" → "Flow · Dim", not "Flow · Flow Dim".
        let label = variant.label.as_str();
        let short = label
            .get(..name.len())
            .filter(|head| head.eq_ignore_ascii_case(name))
            .map_or(label, |_| label[name.len()..].trim_start_matches([' ', '-', ':']))
            .trim();
        format!("{name} · {}", if short.is_empty() { label } else { short })
    } else {
        name.to_string()
    }
}

/// Picker id of the variant the live icon index uses, if any.
fn active_variant_id(app: &App, inventory: &IconInventory) -> Option<String> {
    let index = app.icon_theme.as_ref()?;
    let path = index.default_file.as_ref().or(index.default_folder.as_ref())?.to_string_lossy();
    let (slug, rel) = inventory.active.as_ref()?;
    path.contains(slug.as_str()).then(|| format!("{VARIANT_PREFIX}{slug}|{rel}"))
}

const CHAR_ACTIONS: [(CharacterKeyAction, &str, &str); 3] = [
    (CharacterKeyAction::TypeInFilter, "filter", "sp_type_filter"),
    (CharacterKeyAction::SelectTypedItem, "select", "sp_type_select"),
    (CharacterKeyAction::TriggerHotkey, "hotkey", "sp_type_hotkey"),
];

/// Fonts offered first in the family picker.
/// Listed first in the theme picker with a "Recommended" badge.
const RECOMMENDED_THEME: &str = "Copper Haze Glass";
const RECOMMENDED_FONTS: [&str; 4] = ["Geist", "Segoe UI", "Segoe UI Variable", ".SystemUIFont"];

/// Resize presets offered for the Actions ring (px, longest edge).
const RING_EDGES: [u32; 5] = [800, 1280, 1920, 2560, 3840];
/// JPEG quality presets for the Compress action.
const RING_QUALITIES: [(u8, &str); 3] = [
    (90, "sp_ring_quality_high"),
    (80, "sp_ring_quality_medium"),
    (60, "sp_ring_quality_low"),
];

fn t(key: &str) -> SharedString {
    rust_i18n::t!(key).to_string().into()
}

fn open_url(url: &'static str) {
    if url == crate::ui::overlays::legal::NOTICES_URL {
        crate::ui::overlays::legal::open_notices();
    } else {
        let _ = open::that(url);
    }
}

fn theme_swatch(config: &gpui_component::ThemeConfig) -> Option<[Hsla; 3]> {
    let parse = |value: Option<&SharedString>| {
        value.and_then(|hex| gpui_component::try_parse_color(hex).ok())
    };
    Some([
        parse(config.colors.background.as_ref())?,
        parse(config.colors.primary.as_ref()).or_else(|| parse(config.colors.accent.as_ref()))?,
        parse(config.colors.foreground.as_ref())?,
    ])
}

macro_rules! link {
    ($id:literal, $icon:expr, $title:literal, $desc:expr, $kw:literal, $url:expr) => {
        SettingDef {
            id: $id,
            category: Category::About,
            icon: $icon,
            title: $title,
            desc: $desc,
            keywords: $kw,
            control: Control::Action {
                run: |_| open_url($url),
                external: true,
            },
        }
    };
}

pub(crate) static SETTINGS: LazyLock<Vec<SettingDef>> = LazyLock::new(|| {
    let mut list = vec![
        SettingDef { id: "photo_information", category: Category::Files, icon: AtlasIcon::Info,
            title: "photo_information", desc: Some("information_level_scope"), keywords: "metadata information off standard extended photo raw video audio",
            control: Control::Segmented { labels: || vec!["Off".into(), "Standard".into(), "Extended".into()], icons: &[],
                get: |app| app.state.settings.photo_information.index(),
                set: |app, value| app.state.settings.photo_information = atlas_model::settings::InformationLevel::from_index(value),
                default: Some(|settings| settings.photo_information.index()) },
        },
        SettingDef { id: "media_information", category: Category::Files, icon: AtlasIcon::Info,
            title: "media_information", desc: Some("information_level_scope"), keywords: "metadata information off standard extended photo raw video audio",
            control: Control::Segmented { labels: || vec!["Off".into(), "Standard".into(), "Extended".into()], icons: &[],
                get: |app| app.state.settings.media_information.index(),
                set: |app, value| app.state.settings.media_information = atlas_model::settings::InformationLevel::from_index(value),
                default: Some(|settings| settings.media_information.index()) },
        },
        SettingDef { id: "other_information", category: Category::Files, icon: AtlasIcon::Info,
            title: "other_information", desc: Some("information_level_scope"), keywords: "metadata information off standard extended photo raw video audio",
            control: Control::Segmented { labels: || vec!["Off".into(), "Standard".into(), "Extended".into()], icons: &[],
                get: |app| app.state.settings.other_information.index(),
                set: |app, value| app.state.settings.other_information = atlas_model::settings::InformationLevel::from_index(value),
                default: Some(|settings| settings.other_information.index()) },
        },

        SettingDef { id: "automatic_folder_statistics", category: Category::FolderStatistics,
            icon: AtlasIcon::FolderMeasure, title: "automatic_folder_statistics", desc: None,
            keywords: "automatic folder statistics size folders files SSD HDD network scan",
            control: Control::Toggle { get: |app| app.state.settings.automatic_folder_statistics,
                set: |app, value| app.state.settings.automatic_folder_statistics = value,
                default: Some(|settings| settings.automatic_folder_statistics) },
        },
        SettingDef { id: "folder_statistics_ssd", category: Category::FolderStatistics,
            icon: AtlasIcon::FolderMeasure, title: "folder_statistics_ssd", desc: None,
            keywords: "automatic folder statistics size folders files SSD HDD network scan",
            control: Control::Toggle { get: |app| app.state.settings.folder_statistics_ssd,
                set: |app, value| app.state.settings.folder_statistics_ssd = value,
                default: Some(|settings| settings.folder_statistics_ssd) },
        },
        SettingDef { id: "folder_statistics_hdd", category: Category::FolderStatistics,
            icon: AtlasIcon::FolderMeasure, title: "folder_statistics_hdd", desc: Some("folder_statistics_hdd_policy"),
            keywords: "automatic folder statistics size folders files SSD HDD network scan",
            control: Control::Toggle { get: |app| app.state.settings.folder_statistics_hdd,
                set: |app, value| app.state.settings.folder_statistics_hdd = value,
                default: Some(|settings| settings.folder_statistics_hdd) },
        },
        SettingDef { id: "folder_statistics_network", category: Category::FolderStatistics,
            icon: AtlasIcon::FolderMeasure, title: "folder_statistics_network", desc: None,
            keywords: "automatic folder statistics size folders files SSD HDD network scan",
            control: Control::Toggle { get: |app| app.state.settings.folder_statistics_network,
                set: |app, value| app.state.settings.folder_statistics_network = value,
                default: Some(|settings| settings.folder_statistics_network) },
        },
        // ── General ─────────────────────────────────────────────────────────
        SettingDef {
            id: "language",
            category: Category::General,
            icon: AtlasIcon::Translate,
            title: "settings_language",
            desc: Some("sp_language_desc"),
            keywords: "idioma language langue sprache locale english español",
            control: Control::Choice {
                options: |_, _| {
                    LANGUAGES
                        .iter()
                        .map(|(code, name)| {
                            let mut option = ChoiceOption::new(*code, *name);
                            option.detail = Some((*code).into());
                            option
                        })
                        .collect()
                },
                current: |app| {
                    app.settings_ui
                        .preview_before
                        .clone()
                        .filter(|_| app.settings_ui.drill.as_ref().is_some_and(|d| d.setting == "language"))
                        .or_else(|| app.state.settings.language.clone())
                        .unwrap_or_else(|| "en".into())
                        .into()
                },
                pick: |app, code| {
                    app.settings_ui.preview_before = None;
                    app.state.settings.language = Some(code.to_string());
                    rust_i18n::set_locale(code);
                },
                preview: Some(|app, code| match code {
                    Some(code) => {
                        if app.settings_ui.preview_before.is_none() {
                            app.settings_ui.preview_before = Some(rust_i18n::locale().to_string());
                        }
                        rust_i18n::set_locale(code);
                    }
                    None => {
                        if let Some(before) = app.settings_ui.preview_before.take() {
                            rust_i18n::set_locale(&before);
                        }
                    }
                }),
                default: None,
            },
        },
        SettingDef {
            id: "zoom",
            category: Category::General,
            icon: AtlasIcon::MagnifyingGlassPlus,
            title: "settings_zoom",
            desc: Some("sp_zoom_desc"),
            keywords: "zoom scale escala tamaño interfaz ui",
            control: Control::Slider(Slider {
                min: 0.5,
                max: 3.0,
                step: 0.1,
                unit: Unit::Percent,
                get: |app| app.state.settings.global_zoom,
                set: |app, value| {
                    app.state.settings.global_zoom = value;
                    app.theme_dirty = true;
                },
                default: Some(|s| s.global_zoom),
                live: true,
            }),
        },
        SettingDef {
            id: "autosave-session",
            category: Category::General,
            icon: AtlasIcon::ClockCounterClockwise,
            title: "settings_autosave_session",
            desc: Some("sp_autosave_desc"),
            keywords: "autosave session sesion restaurar restore tabs pestañas",
            control: Control::Toggle {
                get: |app| app.state.settings.autosave_session,
                set: |app, on| app.state.settings.autosave_session = on,
                default: Some(|s| s.autosave_session),
            },
        },
        SettingDef {
            id: "shortcuts",
            category: Category::General,
            icon: AtlasIcon::Command,
            title: "sp_shortcuts",
            desc: Some("sp_shortcuts_desc"),
            keywords: "shortcuts atajos keybindings teclas hotkeys remap reasignar combinacion",
            control: Control::Page { searchable: true },
        },
        // ── Appearance ──────────────────────────────────────────────────────
        SettingDef {
            id: "theme",
            category: Category::Appearance,
            icon: AtlasIcon::Palette,
            title: "settings_theme_preset",
            desc: Some("sp_theme_desc"),
            keywords: "theme tema color colores dark light oscuro claro glass",
            control: Control::Choice {
                options: |_, cx| {
                    let registry = gpui_component::ThemeRegistry::global(cx);
                    let mut options: Vec<ChoiceOption> = registry
                        .themes()
                        .iter()
                        .filter(|(name, _)| crate::app::theme::theme_is_available(name))
                        .map(|(name, config)| {
                            let mut option = ChoiceOption::new(name.clone(), atlas_core::util::display::theme_label(name));
                            option.swatch = theme_swatch(config);
                            if name.as_ref() == RECOMMENDED_THEME {
                                option.detail = Some(t("sp_recommended"));
                            }
                            option
                        })
                        .collect();
                    options.sort_by(|a, b| {
                        (a.id != RECOMMENDED_THEME, &a.label).cmp(&(b.id != RECOMMENDED_THEME, &b.label))
                    });
                    options
                },
                current: |app| {
                    app.theme_preview_before
                        .clone()
                        .unwrap_or_else(|| app.state.profiles.active().theme.clone())
                        .into()
                },
                pick: |app, name| {
                    app.settings_theme_previewing = false;
                    app.set_theme(name.to_string());
                },
                preview: Some(|app, name| match name {
                    Some(name) => {
                        app.settings_theme_previewing = true;
                        app.preview_theme(name.to_string());
                    }
                    None => {
                        app.settings_theme_previewing = false;
                        app.restore_theme_preview();
                    }
                }),
                default: Some(|_| atlas_model::profiles::DEFAULT_THEME.into()),
            },
        },
        SettingDef {
            id: "theme-editor",
            category: Category::Appearance,
            icon: AtlasIcon::PencilSimple,
            title: "sp_theme_edit",
            desc: Some("theme_fork_description"),
            keywords: "theme editor fork duplicate duplicar personalizar custom colores",
            control: Control::Page { searchable: false },
        },
        SettingDef {
            id: "themes-folder",
            category: Category::Appearance,
            icon: AtlasIcon::FolderOpen,
            title: "theme_open_folder",
            desc: Some("sp_themes_folder_desc"),
            keywords: "themes folder carpeta temas import importar json",
            control: Control::Action {
                // Opens behind the panel; Settings stays where the user is.
                run: |app| {
                    let path = atlas_core::util::config::themes_dir();
                    let pane = app.state.focused_tile;
                    app.process_actions([Action::NewTabWithPath { pane, path }]);
                    app.toast_info(t("sp_themes_folder_opened").to_string());
                },
                external: false,
            },
        },
        SettingDef {
            id: "icon-style",
            category: Category::Appearance,
            icon: AtlasIcon::File,
            title: "settings_icon_style",
            desc: Some("sp_icon_style_desc"),
            keywords: "icons iconos file archivo windows theme tema estilo download descargar material catppuccin flow",
            // One picker for every icon source: the built-in styles plus the
            // downloadable themes (picking a missing one downloads + activates it).
            control: Control::Choice {
                options: |app, _| {
                    let inventory = icon_inventory();
                    let active = active_variant_id(app, &inventory);
                    let style = app.icon_style_preview_before.unwrap_or(app.state.settings.icon_style);
                    let mut options: Vec<ChoiceOption> = ICON_STYLES
                        .iter()
                        .filter(|(s, ..)| *s != IconStyle::Theme || (style == IconStyle::Theme && active.is_none()))
                        .map(|(_, id, key)| ChoiceOption::new(*id, t(key)))
                        .collect();
                    // Installed themes, one entry per variant (Catppuccin Mocha, Latte…).
                    options.extend(inventory.variants.iter().map(|variant| {
                        let id = variant_id(variant);
                        let mut option = ChoiceOption::new(id.clone(), variant_label(variant));
                        if active.as_deref() == Some(id.as_str()) {
                            option.detail = Some(t("sp_active"));
                        }
                        option
                    }));
                    // Known themes not downloaded yet.
                    options.extend(
                        ICON_THEME_SOURCES
                            .iter()
                            .filter(|(_, _, slug)| !inventory.variants.iter().any(|v| v.slug == *slug))
                            .map(|(cmd, label, _)| {
                                let mut option = ChoiceOption::new(*cmd, *label);
                                option.detail = Some(t("sp_download"));
                                option
                            }),
                    );
                    options
                },
                current: |app| {
                    let style = app.icon_style_preview_before.unwrap_or(app.state.settings.icon_style);
                    match (style, active_variant_id(app, &icon_inventory())) {
                        (IconStyle::Theme, Some(id)) => id.into(),
                        (style, _) => icon_style_id(style).into(),
                    }
                },
                pick: |app, id| {
                    let active = active_variant_id(app, &icon_inventory());
                    let style = match icon_style_from_id(id) {
                        Some(style) => style,
                        None if active.as_deref() == Some(id) => IconStyle::Theme,
                        None => {
                            app.restore_icon_style_preview();
                            if let Some((slug, rel)) = id.strip_prefix(VARIANT_PREFIX).and_then(|v| v.split_once('|')) {
                                // Installed: switch variants in the background; the
                                // usual completion path persists + applies it.
                                ICON_INVENTORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).active =
                                    Some((slug.to_string(), rel.to_string()));
                                atlas_core::ext::runner::activate_icon_variant(
                                    &app.rt,
                                    app.tx.clone(),
                                    icon_data_dir(),
                                    slug.to_string(),
                                    rel.to_string(),
                                );
                            } else {
                                // Not installed: the download activates it on completion.
                                app.process_actions([Action::DownloadIconTheme(id.to_string())]);
                            }
                            return;
                        }
                    };
                    app.icon_style_preview_before = None;
                    if app.state.settings.icon_style != style {
                        app.state.settings.icon_style = style;
                        app.apply_profile_icons();
                        app.relist_panes(|_| true);
                    }
                },
                preview: Some(|app, id| {
                    let active = active_variant_id(app, &icon_inventory());
                    let style = id.and_then(|id| {
                        icon_style_from_id(id).or_else(|| (active.as_deref() == Some(id)).then_some(IconStyle::Theme))
                    });
                    match style {
                        Some(style) => app.preview_icon_style(style),
                        None => app.restore_icon_style_preview(),
                    }
                }),
                default: Some(|s| icon_style_id(s.icon_style).into()),
            },
        },
        SettingDef {
            id: "font-family",
            category: Category::Appearance,
            icon: AtlasIcon::TextAa,
            title: "sp_font_family",
            desc: Some("sp_font_family_desc"),
            keywords: "font fuente tipografia typeface family familia letra",
            control: Control::Choice {
                options: |app, cx| {
                    let fonts = app
                        .settings_font_families
                        .get_or_insert_with(|| {
                            let mut names = cx.text_system().all_font_names();
                            names.sort_by_key(|name| name.to_lowercase());
                            names.dedup();
                            std::sync::Arc::new(names.into_iter().map(SharedString::from).collect())
                        })
                        .clone();
                    let recommended = t("sp_recommended");
                    let mut options: Vec<ChoiceOption> = RECOMMENDED_FONTS
                        .iter()
                        .map(|name| {
                            let label = if *name == ".SystemUIFont" { t("sp_font_system") } else { (*name).into() };
                            let mut option = ChoiceOption::new(*name, label);
                            option.detail = Some(recommended.clone());
                            option.font = Some((*name).into());
                            option
                        })
                        .collect();
                    options.extend(
                        fonts
                            .iter()
                            .filter(|name| !RECOMMENDED_FONTS.contains(&name.as_ref()) && !name.starts_with('.'))
                            .map(|name| {
                                let mut option = ChoiceOption::new(name.clone(), name.clone());
                                option.font = Some(name.clone());
                                option
                            }),
                    );
                    options
                },
                current: |app| {
                    app.settings_ui
                        .preview_before
                        .clone()
                        .filter(|_| app.settings_ui.drill.as_ref().is_some_and(|d| d.setting == "font-family"))
                        .unwrap_or_else(|| app.state.settings.font_family.clone())
                        .into()
                },
                pick: |app, name| {
                    app.settings_ui.preview_before = None;
                    app.state.settings.font_family = name.to_string();
                },
                preview: Some(|app, name| match name {
                    Some(name) => {
                        if app.settings_ui.preview_before.is_none() {
                            app.settings_ui.preview_before = Some(app.state.settings.font_family.clone());
                        }
                        app.state.settings.font_family = name.to_string();
                    }
                    None => {
                        if let Some(before) = app.settings_ui.preview_before.take() {
                            app.state.settings.font_family = before;
                        }
                    }
                }),
                default: Some(|s| s.font_family.clone().into()),
            },
        },
        SettingDef {
            id: "font-size",
            category: Category::Appearance,
            icon: AtlasIcon::TextT,
            title: "sp_font_size",
            desc: Some("sp_font_size_desc"),
            keywords: "font size tamaño fuente texto text letra",
            control: Control::Slider(Slider {
                min: 10.0,
                max: 24.0,
                step: 1.0,
                unit: Unit::Px,
                get: |app| app.state.settings.font_size,
                set: |app, value| app.state.settings.font_size = value,
                default: Some(|s| s.font_size),
                live: true,
            }),
        },
        SettingDef {
            id: "shortcut-target-opacity", category: Category::Appearance,
            icon: AtlasIcon::File, title: "settings_shortcut_target_opacity",
            desc: Some("settings_shortcut_target_opacity_desc"), keywords: "shortcut lnk target destination opacity",
            control: Control::Slider(Slider {
                min: 0.0, max: 0.60, step: 0.01, unit: Unit::Percent,
                get: |app| atlas_model::settings::shortcut_alpha(app.state.settings.shortcut_target_opacity),
                set: |app, value| {
                    app.state.settings.shortcut_target_opacity = atlas_model::settings::shortcut_alpha(value);
                    if value == 0.0 { app.context_info.shortcuts.cancel(); }
                },
                default: Some(|s| s.shortcut_target_opacity), live: true,
            }),
        },
        SettingDef {
            id: "file-selection-opacity", category: Category::Appearance,
            icon: AtlasIcon::SelectionAll, title: "settings_file_selection_opacity",
            desc: Some("settings_file_selection_opacity_desc"), keywords: "file folder selection selected background opacity highlight",
            control: Control::Slider(Slider {
                min: 0.10, max: 0.70, step: 0.01, unit: Unit::Percent,
                get: |app| app.state.settings.file_selection_alpha(),
                set: |app, value| app.state.settings.file_selection_opacity = atlas_model::settings::selection_alpha(value),
                default: Some(|s| s.file_selection_opacity), live: true,
            }),
        },
        SettingDef {
            id: "detail-column-separators", category: Category::Appearance,
            icon: AtlasIcon::Columns, title: "settings_detail_separators",
            desc: Some("settings_detail_separators_desc"), keywords: "details table columns separators lines borders",
            control: Control::Toggle {
                get: |app| app.state.settings.detail_column_separators,
                set: |app, on| app.state.settings.detail_column_separators = on,
                default: Some(|s| s.detail_column_separators),
            },
        },
        SettingDef {
            id: "detail-separator-strength", category: Category::Appearance,
            icon: AtlasIcon::Columns, title: "settings_detail_separator_strength",
            desc: Some("settings_detail_separators_desc"), keywords: "details table columns separators strength opacity",
            control: Control::Slider(Slider {
                min: 0.0, max: 1.0, step: 0.01, unit: Unit::Percent,
                get: |app| app.state.settings.detail_separator_strength,
                set: |app, value| app.state.settings.detail_separator_strength = value,
                default: Some(|s| s.detail_separator_strength), live: true,
            }),
        },
        SettingDef {
            id: "alternating-row-colors", category: Category::Appearance,
            icon: AtlasIcon::List, title: "settings_alternating_rows",
            desc: Some("settings_alternating_rows_desc"), keywords: "list zebra stripes alternating row colors",
            control: Control::Toggle {
                get: |app| app.state.settings.alternating_row_colors,
                set: |app, on| app.state.settings.alternating_row_colors = on,
                default: Some(|s| s.alternating_row_colors),
            },
        },
        SettingDef {
            id: "alternating-row-strength", category: Category::Appearance,
            icon: AtlasIcon::List, title: "settings_alternating_strength",
            desc: Some("settings_alternating_rows_desc"), keywords: "list zebra stripes alternating strength",
            control: Control::Slider(Slider {
                min: 0.0, max: 1.0, step: 0.01, unit: Unit::Percent,
                get: |app| app.state.settings.alternating_row_strength,
                set: |app, value| app.state.settings.alternating_row_strength = value,
                default: Some(|s| s.alternating_row_strength), live: true,
            }),
        },
        SettingDef {
            id: "glass-opacity",
            category: Category::Appearance,
            icon: AtlasIcon::Drop,
            title: "settings_glass_opacity",
            desc: Some("settings_glass_opacity_desc"),
            keywords: "glass cristal transparency transparencia opacity opacidad blur desenfoque",
            control: Control::Slider(Slider {
                min: 0.2,
                max: 1.0,
                step: 0.05,
                unit: Unit::Percent,
                get: |app| app.state.settings.glass_opacity,
                set: |app, value| {
                    app.state.settings.glass_opacity = value;
                    app.theme_dirty = true;
                },
                default: Some(|s| s.glass_opacity),
                live: true,
            }),
        },
        SettingDef {
            id: "animations",
            category: Category::Appearance,
            icon: AtlasIcon::Sparkle,
            title: "sp_animations",
            desc: Some("sp_animations_desc"),
            keywords: "animations animaciones motion movimiento disable desactivar reduce",
            control: Control::Toggle {
                get: |app| !app.state.settings.disable_animations,
                set: |app, on| app.state.settings.disable_animations = !on,
                default: Some(|s| !s.disable_animations),
            },
        },
        SettingDef {
            id: "titlebar",
            category: Category::Appearance,
            icon: AtlasIcon::AppWindow,
            title: "settings_show_titlebar",
            desc: Some("sp_titlebar_desc"),
            keywords: "titlebar title bar window barra ventana chrome",
            control: Control::Toggle {
                get: |app| app.state.settings.show_titlebar,
                set: |app, on| app.state.settings.show_titlebar = on,
                default: Some(|s| s.show_titlebar),
            },
        },
        SettingDef {
            id: "dim-inactive-panes",
            category: Category::Appearance,
            icon: AtlasIcon::SplitHorizontal,
            title: "settings_dim_inactive_panes",
            desc: Some("sp_dim_desc"),
            keywords: "dim inactive pane atenuar panel inactivo focus foco",
            control: Control::Toggle {
                get: |app| app.state.settings.dim_inactive_panes,
                set: |app, on| app.state.settings.dim_inactive_panes = on,
                default: Some(|s| s.dim_inactive_panes),
            },
        },
        // ── Files & navigation ──────────────────────────────────────────────
        SettingDef {
            id: "default-view",
            category: Category::Files,
            icon: AtlasIcon::Details,
            title: "sp_default_view",
            desc: Some("sp_default_view_desc"),
            keywords: "view vista details detalles grid iconos list lista miller columnas tree arbol",
            control: Control::Segmented {
                labels: || VIEW_MODES.iter().map(|(_, key, _)| t(key)).collect(),
                icons: &VIEW_ICONS,
                get: |app| VIEW_MODES.iter().position(|(mode, ..)| *mode == app.state.settings.view_mode).unwrap_or(0),
                set: |app, idx| {
                    if let Some((mode, ..)) = VIEW_MODES.get(idx) {
                        app.state.settings.view_mode = mode.clone();
                    }
                },
                default: Some(|s| VIEW_MODES.iter().position(|(mode, ..)| *mode == s.view_mode).unwrap_or(0)),
            },
        },
        SettingDef {
            id: "show-known-extensions",
            category: Category::Files,
            icon: AtlasIcon::Details,
            title: "Show known file extensions",
            desc: None,
            keywords: "show hide extensions filenames file types suffix explorer raw cr2 psd zip",
            control: Control::Toggle {
                get: |app| app.state.settings.show_known_file_extensions,
                set: |app, on| app.state.settings.show_known_file_extensions = on,
                default: Some(|s| s.show_known_file_extensions),
            },
        },
        SettingDef {
            id: "details-row-spacing",
            category: Category::Files,
            icon: AtlasIcon::Details,
            title: "details_view_spacing",
            desc: None,
            keywords: "details view row spacing density compact normal comfortable lines height list",
            control: Control::Segmented {
                labels: || vec!["Normal".into(), "Comfortable".into()],
                icons: &[],
                get: |app| usize::from(app.state.settings.comfortable_details_rows),
                set: |app, index| app.state.settings.comfortable_details_rows = index == 1,
                default: Some(|s| usize::from(s.comfortable_details_rows)),
            },
        },
        SettingDef {
            id: "show-hidden",
            category: Category::Files,
            icon: AtlasIcon::Eye,
            title: "sp_show_hidden",
            desc: Some("sp_show_hidden_desc"),
            keywords: "hidden ocultos show mostrar dotfiles system sistema",
            control: Control::Toggle {
                get: |app| app.state.settings.show_hidden,
                set: |app, on| {
                    if app.state.settings.show_hidden != on {
                        app.process_actions([Action::ToggleHidden]);
                    }
                },
                default: Some(|s| s.show_hidden),
            },
        },
        SettingDef {
            id: "single-click-folders",
            category: Category::Files,
            icon: AtlasIcon::CursorClick,
            title: "sp_single_click",
            desc: Some("settings_single_click_folders"),
            keywords: "single click clic carpetas folders abrir open navigation",
            control: Control::Toggle {
                get: |app| app.state.settings.single_click_folders_only,
                set: |app, on| {
                    app.state.settings.single_click_folders_only = on;
                    app.pending_folder_click = None;
                },
                default: Some(|s| s.single_click_folders_only),
            },
        },
        SettingDef {
            id: "character-keys",
            category: Category::Files,
            icon: AtlasIcon::Keyboard,
            title: "settings_character_keys",
            desc: Some("sp_character_keys_desc"),
            keywords: "typing teclear type escribir filter filtro select seleccionar hotkey",
            control: Control::Segmented {
                labels: || CHAR_ACTIONS.iter().map(|(_, _, key)| t(key)).collect(),
                icons: &[],
                get: |app| CHAR_ACTIONS.iter().position(|(action, ..)| *action == app.state.settings.character_key_action).unwrap_or(0),
                set: |app, idx| {
                    if let Some((action, ..)) = CHAR_ACTIONS.get(idx) {
                        app.state.settings.character_key_action = *action;
                        app.typed_selection = None;
                    }
                },
                default: Some(|s| CHAR_ACTIONS.iter().position(|(action, ..)| *action == s.character_key_action).unwrap_or(0)),
            },
        },
        SettingDef {
            id: "filter-sticky",
            category: Category::Files,
            icon: AtlasIcon::Funnel,
            title: "settings_filter_sticky",
            desc: None,
            keywords: "filter filtro sticky persistente keep mantener",
            control: Control::Toggle {
                get: |app| app.state.settings.filter_sticky,
                set: |app, on| app.state.settings.filter_sticky = on,
                default: Some(|s| s.filter_sticky),
            },
        },
        SettingDef {
            id: "vim-mode",
            category: Category::Files,
            icon: AtlasIcon::TerminalWindow,
            title: "sp_vim",
            desc: Some("sp_vim_desc"),
            keywords: "vim hjkl keys teclas navigation navegacion",
            control: Control::Toggle {
                get: |app| app.state.settings.vim_mode,
                set: |app, on| {
                    if app.state.settings.vim_mode != on {
                        app.process_actions([Action::ToggleVimMode]);
                    }
                },
                default: Some(|s| s.vim_mode),
            },
        },
        SettingDef {
            id: "scroll-speed",
            category: Category::Files,
            icon: AtlasIcon::SlidersHorizontal,
            title: "settings_scroll_speed",
            desc: Some("sp_scroll_speed_desc"),
            keywords: "scroll wheel speed distance",
            control: Control::Slider(Slider {
                min: 0.25, max: 3.0, step: 0.25, unit: Unit::Percent,
                get: |app| app.state.settings.scroll_speed,
                set: |app, value| app.state.settings.scroll_speed = value,
                default: Some(|s| s.scroll_speed), live: true,
            }),
        },
        SettingDef {
            id: "scroll-smoothness",
            category: Category::Files,
            icon: AtlasIcon::SlidersHorizontal,
            title: "settings_scroll_smoothness",
            desc: Some("sp_smooth_desc"),
            keywords: "smooth suave scroll desplazamiento suavidad inercia",
            control: Control::Slider(Slider {
                min: 0.0,
                max: 1.0,
                step: 0.1,
                unit: Unit::Percent,
                get: |app| app.state.settings.scroll_smoothness,
                set: |app, value| app.state.settings.scroll_smoothness = value,
                default: Some(|s| s.scroll_smoothness),
                live: true,
            }),
        },
        SettingDef {
            id: "scroll-overscroll",
            category: Category::Files,
            icon: AtlasIcon::ArrowsVertical,
            title: "settings_scroll_beyond_last",
            desc: Some("sp_overscroll_desc"),
            keywords: "overscroll beyond scroll margen ultimo last espacio",
            control: Control::Slider(Slider {
                min: 0.0,
                max: 0.9,
                step: 0.1,
                unit: Unit::Percent,
                get: |app| app.state.settings.scroll_overscroll_fraction(),
                set: |app, value| app.state.settings.scroll_overscroll = value,
                default: Some(Settings::scroll_overscroll_fraction),
                live: true,
            }),
        },
        // ── Convert / Actions ring ──────────────────────────────────────────
        SettingDef {
            id: "ring-open",
            category: Category::Files,
            icon: AtlasIcon::ArrowsLeftRight,
            title: "sp_ring_open",
            desc: Some("sp_ring_open_desc"),
            keywords: "ring anillo convert convertir actions acciones radial menu wheel rueda",
            control: Control::Segmented {
                labels: || vec![t("ring_mode_convert"), t("ring_mode_actions")],
                icons: &[],
                get: |app| usize::from(app.state.settings.ring.open_actions),
                set: |app, idx| app.state.settings.ring.open_actions = idx == 1,
                default: Some(|s| usize::from(s.ring.open_actions)),
            },
        },
        SettingDef {
            id: "ring-resize-edge",
            category: Category::Files,
            icon: AtlasIcon::Image,
            title: "sp_ring_resize_edge",
            desc: Some("sp_ring_resize_edge_desc"),
            keywords: "resize redimensionar tamaño size pixels px imagen image ring anillo",
            control: Control::Segmented {
                labels: || RING_EDGES.iter().map(|edge| SharedString::from(format!("{edge}"))).collect(),
                icons: &[],
                get: |app| RING_EDGES.iter().position(|edge| *edge == app.state.settings.ring.resize_edge).unwrap_or(2),
                set: |app, idx| {
                    if let Some(edge) = RING_EDGES.get(idx) {
                        app.state.settings.ring.resize_edge = *edge;
                    }
                },
                default: Some(|s| RING_EDGES.iter().position(|edge| *edge == s.ring.resize_edge).unwrap_or(2)),
            },
        },
        SettingDef {
            id: "ring-quality",
            category: Category::Files,
            icon: AtlasIcon::SlidersHorizontal,
            title: "sp_ring_quality",
            desc: Some("sp_ring_quality_desc"),
            keywords: "compress comprimir quality calidad jpeg jpg ring anillo",
            control: Control::Segmented {
                labels: || RING_QUALITIES.iter().map(|(_, key)| t(key)).collect(),
                icons: &[],
                get: |app| RING_QUALITIES.iter().position(|(q, _)| *q == app.state.settings.ring.compress_quality).unwrap_or(1),
                set: |app, idx| {
                    if let Some((quality, _)) = RING_QUALITIES.get(idx) {
                        app.state.settings.ring.compress_quality = *quality;
                    }
                },
                default: Some(|s| RING_QUALITIES.iter().position(|(q, _)| *q == s.ring.compress_quality).unwrap_or(1)),
            },
        },
        SettingDef {
            id: "ring-show-resize",
            category: Category::Files,
            icon: AtlasIcon::Image,
            title: "sp_ring_show_resize",
            desc: Some("sp_ring_show_desc"),
            keywords: "ring anillo actions acciones show mostrar hide ocultar",
            control: Control::Toggle {
                get: |app| app.state.settings.ring.shows("resize"),
                set: |app, on| app.state.settings.ring.set_shown("resize", on),
                default: Some(|s| s.ring.shows("resize")),
            },
        },
        SettingDef {
            id: "ring-show-rotate-right",
            category: Category::Files,
            icon: AtlasIcon::ArrowClockwise,
            title: "sp_ring_show_rotate_right",
            desc: Some("sp_ring_show_desc"),
            keywords: "ring anillo actions acciones show mostrar hide ocultar",
            control: Control::Toggle {
                get: |app| app.state.settings.ring.shows("rotate-right"),
                set: |app, on| app.state.settings.ring.set_shown("rotate-right", on),
                default: Some(|s| s.ring.shows("rotate-right")),
            },
        },
        SettingDef {
            id: "ring-show-rotate-left",
            category: Category::Files,
            icon: AtlasIcon::ArrowCounterClockwise,
            title: "sp_ring_show_rotate_left",
            desc: Some("sp_ring_show_desc"),
            keywords: "ring anillo actions acciones show mostrar hide ocultar",
            control: Control::Toggle {
                get: |app| app.state.settings.ring.shows("rotate-left"),
                set: |app, on| app.state.settings.ring.set_shown("rotate-left", on),
                default: Some(|s| s.ring.shows("rotate-left")),
            },
        },
        SettingDef {
            id: "ring-show-strip-metadata",
            category: Category::Files,
            icon: AtlasIcon::Tag,
            title: "sp_ring_show_strip",
            desc: Some("sp_ring_show_desc"),
            keywords: "ring anillo actions acciones show mostrar hide ocultar",
            control: Control::Toggle {
                get: |app| app.state.settings.ring.shows("strip-metadata"),
                set: |app, on| app.state.settings.ring.set_shown("strip-metadata", on),
                default: Some(|s| s.ring.shows("strip-metadata")),
            },
        },
        SettingDef {
            id: "ring-show-compress",
            category: Category::Files,
            icon: AtlasIcon::TrayArrowDown,
            title: "sp_ring_show_compress",
            desc: Some("sp_ring_show_desc"),
            keywords: "ring anillo actions acciones show mostrar hide ocultar",
            control: Control::Toggle {
                get: |app| app.state.settings.ring.shows("compress"),
                set: |app, on| app.state.settings.ring.set_shown("compress", on),
                default: Some(|s| s.ring.shows("compress")),
            },
        },
        SettingDef {
            id: "ring-show-compress-pdf",
            category: Category::Files,
            icon: AtlasIcon::File,
            title: "sp_ring_show_compress_pdf",
            desc: Some("sp_ring_show_desc"),
            keywords: "ring anillo actions acciones show mostrar hide ocultar",
            control: Control::Toggle {
                get: |app| app.state.settings.ring.shows("compress-pdf"),
                set: |app, on| app.state.settings.ring.set_shown("compress-pdf", on),
                default: Some(|s| s.ring.shows("compress-pdf")),
            },
        },
        // ── Layout ──────────────────────────────────────────────────────────
        SettingDef {
            id: "show-sidebar",
            category: Category::Layout,
            icon: AtlasIcon::SidebarSimple,
            title: "sp_show_sidebar",
            desc: None,
            keywords: "sidebar barra lateral panel show mostrar",
            control: Control::Toggle {
                get: |app| app.state.settings.show_sidebar,
                set: |app, on| {
                    if app.state.settings.show_sidebar != on {
                        app.process_actions([Action::ToggleSidebar]);
                    }
                },
                default: Some(|s| s.show_sidebar),
            },
        },
        SettingDef {
            id: "sidebar-width",
            category: Category::Layout,
            icon: AtlasIcon::ArrowsLeftRight,
            title: "sp_sidebar_width",
            desc: None,
            keywords: "sidebar width ancho barra lateral",
            control: Control::Slider(Slider {
                min: 180.0,
                max: 420.0,
                step: 10.0,
                unit: Unit::Px,
                get: |app| app.state.settings.sidebar_width,
                set: |app, value| {
                    app.state.settings.sidebar_width = value;
                    app.sidebar_width_dirty = true;
                },
                default: Some(|s| s.sidebar_width),
                live: true,
            }),
        },
        SettingDef {
            id: "show-inspector",
            category: Category::Layout,
            icon: AtlasIcon::Monitor,
            title: "settings_show_inspector",
            desc: Some("sp_inspector_desc"),
            keywords: "inspector preview vista previa panel show mostrar",
            control: Control::Toggle {
                get: |app| app.state.settings.show_inspector,
                set: |app, on| app.state.settings.show_inspector = on,
                default: Some(|s| s.show_inspector),
            },
        },
        SettingDef {
            id: "inspector-placement",
            category: Category::Layout,
            icon: AtlasIcon::Columns,
            title: "sp_inspector_placement",
            desc: Some("sp_inspector_placement_desc"),
            keywords: "inspector placement global tab pestaña ubicacion posicion",
            control: Control::Segmented {
                labels: || vec![t("settings_placement_global"), t("settings_placement_tab")],
                icons: &[],
                get: |app| usize::from(app.state.settings.inspector_placement == InspectorPlacement::ActiveTab),
                set: |app, idx| {
                    app.state.settings.inspector_placement = if idx == 0 {
                        InspectorPlacement::Global
                    } else {
                        InspectorPlacement::ActiveTab
                    };
                },
                default: Some(|s| usize::from(s.inspector_placement == InspectorPlacement::ActiveTab)),
            },
        },
        SettingDef {
            id: "inspector-width",
            category: Category::Layout,
            icon: AtlasIcon::SplitVertical,
            title: "sp_inspector_width",
            desc: None,
            keywords: "inspector width ancho preview panel",
            control: Control::Slider(Slider {
                min: 180.0,
                max: 2000.0,
                step: 20.0,
                unit: Unit::Px,
                get: |app| app.state.settings.inspector_width,
                set: |app, value| app.state.settings.inspector_width = value,
                default: Some(|s| s.inspector_width),
                live: true,
            }),
        },
        SettingDef {
            id: "thumbnail-size",
            category: Category::Layout,
            icon: AtlasIcon::Image,
            title: "sp_thumbnail_size",
            desc: None,
            keywords: "thumbnail thumb miniatura size tamaño grid iconos",
            control: Control::Slider(Slider {
                min: 64.0,
                max: 256.0,
                step: 16.0,
                unit: Unit::Px,
                get: |app| app.state.settings.thumbnail_size,
                set: |app, value| app.state.settings.thumbnail_size = value,
                default: Some(|s| s.thumbnail_size),
                live: true,
            }),
        },
        SettingDef {
            id: "grid-spacing",
            category: Category::Layout,
            icon: AtlasIcon::Grid,
            title: "settings_grid_spacing",
            desc: None,
            keywords: "icon spacing grid spacing espaciado gap cuadricula separacion",
            control: Control::Slider(Slider {
                min: 4.0,
                max: 48.0,
                step: 2.0,
                unit: Unit::Px,
                get: |app| app.state.settings.grid_spacing,
                set: |app, value| app.state.settings.grid_spacing = value,
                default: Some(|s| s.grid_spacing),
                live: true,
            }),
        },
        // ── Performance ─────────────────────────────────────────────────────
        SettingDef {
            id: "eco-mode",
            category: Category::Performance,
            icon: AtlasIcon::Leaf,
            title: "sp_eco",
            desc: Some("sp_eco_desc"),
            keywords: "eco ram memory memoria ahorro inactive inactivo",
            control: Control::Toggle {
                get: |app| app.state.settings.eco_mode,
                set: |app, on| app.state.settings.eco_mode = on,
                default: Some(|s| s.eco_mode),
            },
        },
        SettingDef {
            id: "goto-extended-index",
            category: Category::Performance,
            icon: AtlasIcon::Search,
            title: "sp_goto_index",
            desc: Some("sp_goto_index_desc"),
            keywords: "index indexar goto ir wsl drives discos search buscar",
            control: Control::Toggle {
                get: App::goto_extended_index,
                set: |app, on| {
                    if app.goto_extended_index() != on {
                        app.set_goto_extended_index(on);
                    }
                },
                default: Some(|_| bool::default()),
            },
        },
        SettingDef {
            id: "thumbnail-cache",
            category: Category::Performance,
            icon: AtlasIcon::HardDrive,
            title: "sp_thumb_cache",
            desc: Some("sp_thumb_cache_desc"),
            keywords: "thumbnail cache cache miniaturas memoria memory gpu",
            control: Control::Slider(Slider {
                min: 32.0,
                max: 1024.0,
                step: 32.0,
                unit: Unit::Mb,
                get: |app| app.state.settings.thumbnail_cache_mb as f32,
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // reason: snapped 32..=1024
                set: |app, value| app.state.settings.thumbnail_cache_mb = value.round() as usize,
                #[allow(clippy::cast_precision_loss)] // reason: small MB budget
                default: Some(|s| s.thumbnail_cache_mb as f32),
                live: true,
            }),
        },
        // ── About ───────────────────────────────────────────────────────────
        SettingDef {
            id: "version",
            category: Category::About,
            icon: AtlasIcon::Info,
            title: "sp_version",
            desc: None,
            keywords: "about acerca version versión fldr",
            control: Control::Info {
                value: |_| format!("FLDR {}", atlas_model::build_info::VERSION).into(),
            },
        },
        SettingDef {
            id: "build-identity",
            category: Category::About,
            icon: AtlasIcon::GitBranch,
            title: "sp_build_identity",
            desc: None,
            keywords: "build revision source dirty development",
            control: Control::Info { value: |_| format!("{} ({}) · {}", atlas_model::build_info::REVISION, atlas_model::build_info::TREE_STATE, atlas_model::build_info::SOURCE_ID).into() },
        },
        SettingDef {
            id: "build-date",
            category: Category::About,
            icon: AtlasIcon::Info,
            title: "sp_build_date",
            desc: None,
            keywords: "build date time UTC",
            control: Control::Info { value: |_| atlas_model::build_info::BUILD_DATE.into() },
        },
        SettingDef {
            id: "build-configuration",
            category: Category::About,
            icon: AtlasIcon::Info,
            title: "sp_build_configuration",
            desc: None,
            keywords: "build configuration profile target",
            control: Control::Info { value: |_| format!("{} · {}", atlas_model::build_info::BUILD_PROFILE, atlas_model::build_info::BUILD_TARGET).into() },
        },
        SettingDef {
            id: "copy-version",
            category: Category::About,
            icon: AtlasIcon::Info,
            title: "sp_copy_version",
            desc: None,
            keywords: "copy version information clipboard",
            control: Control::Action { run: |app| {
                if let Err(error) = arboard::Clipboard::new().and_then(|mut c| c.set_text(atlas_model::build_info::version_information())) {
                    app.toast_error(format!("Could not copy version information: {error}"));
                }
            }, external: false },
        },
        SettingDef {
            id: "guide",
            category: Category::About,
            icon: AtlasIcon::Lifebuoy,
            title: "guide_open_link",
            desc: None,
            keywords: "guide guia help ayuda tutorial",
            control: Control::Action {
                run: |app| {
                    app.close_settings_state();
                    app.process_actions([Action::OpenGuide(0)]);
                },
                external: false,
            },
        },
        SettingDef {
            id: "fileop-copyright",
            category: Category::About,
            icon: AtlasIcon::Info,
            title: "fileop_modifications",
            desc: None,
            keywords: "copyright attribution digART FLDR",
            control: Control::Info { value: |_| t("fileop_copyright") },
        },
        SettingDef {
            id: "fileop-upstream",
            category: Category::About,
            icon: AtlasIcon::Info,
            title: "fileop_upstream",
            desc: Some("fileop_source_availability"),
            keywords: "upstream atlas AGPL source license",
            control: Control::Info { value: |_| t("fileop_upstream_notice") },
        },
        link!("source", AtlasIcon::GitBranch, "sp_source", None, "source code codigo fuente github", crate::ui::overlays::legal::NOTICES_URL),
        link!("licenses", AtlasIcon::TextFile, "sp_licenses", None, "licenses licencias third party terceros notices", crate::ui::overlays::legal::NOTICES_URL),
        link!("winfsp", AtlasIcon::HardDrives, "sp_winfsp", Some("sp_winfsp_desc"), "winfsp file system proxy", "https://github.com/winfsp/winfsp"),
    ];
    list.push(SettingDef {
        id: "toolbar-labels", category: Category::Layout, icon: AtlasIcon::TextAa,
        title: "toolbar_labels", desc: Some("toolbar_order_help"), keywords: "toolbar icons labels actions commands",
        control: Control::Toggle { get: |app| app.state.settings.toolbar_labels,
            set: |app, value| app.state.settings.toolbar_labels = value,
            default: Some(|settings| settings.toolbar_labels) },
    });
    macro_rules! toolbar_slot {
        ($id:literal, $title:literal, $position:expr) => {
            list.push(SettingDef {
                id: concat!("toolbar-", $id), category: Category::Layout, icon: AtlasIcon::SlidersHorizontal,
                title: $title, desc: Some("toolbar_order_help"), keywords: "toolbar order position hide action",
                control: Control::Slider(Slider { min: 0., max: 12., step: 1., unit: Unit::Order,
                    get: |app| app.state.settings.toolbar_order.iter().position(|id| id == $id).map_or(0., |index| (index + 1) as f32),
                    set: |app, value| {
                        let order = &mut app.state.settings.toolbar_order;
                        order.retain(|id| id != $id);
                        if value >= 0.5 { order.insert(((value.round() as usize).saturating_sub(1)).min(order.len()), $id.to_owned()); }
                    },
                    default: Some(|s| s.toolbar_order.iter().position(|id| id == $id).map_or(0., |i| (i + 1) as f32)), live: false,
                }),
            });
        };
    }
    toolbar_slot!("copy", "toolbar_copy", 1.0);
    toolbar_slot!("cut", "toolbar_cut", 2.0);
    toolbar_slot!("paste", "toolbar_paste", 3.0);
    toolbar_slot!("rename", "cmd_files_rename_title", 4.0);
    toolbar_slot!("delete", "toolbar_delete", 5.0);
    toolbar_slot!("new-folder", "cmd_files_create_folder_title", 6.0);
    toolbar_slot!("new-text-file", "toolbar_new_text", 7.0);
    toolbar_slot!("properties", "root_properties", 8.0);
    toolbar_slot!("terminal", "cmd_files_open_terminal_title", 9.0);
    toolbar_slot!("size", "size_command", 10.0);
    toolbar_slot!("branch", "toolbar_branch", 11.0);
    // Legacy split slots remain readable but no longer render in the top toolbar.
    list.push(SettingDef {
        id: "toolbar-restore", category: Category::Layout, icon: AtlasIcon::ArrowClockwise,
        title: "toolbar_restore", desc: Some("toolbar_restore_desc"), keywords: "toolbar restore reset defaults order visible",
        control: Control::Action { run: |app| {
            let defaults = atlas_model::settings::Settings::default();
            app.state.settings.toolbar_order = defaults.toolbar_order;
            app.state.settings.toolbar_labels = defaults.toolbar_labels;
            app.state.settings.toolbar_new_actions_initialized = true;
            app.state.settings.toolbar_split_initialized = true;
        }, external: false },
    });
    #[cfg(windows)]
    list.insert(
        2,
        SettingDef {
            id: "win-e",
            category: Category::General,
            icon: AtlasIcon::Keyboard,
            title: "settings_win_e",
            desc: Some("sp_win_e_desc"),
            keywords: "win+e explorer explorador default predeterminado windows",
            control: Control::Toggle {
                get: |app| app.win_e_enabled,
                set: |app, on| {
                    if app.win_e_enabled == on {
                        return;
                    }
                    match atlas_win_space::integration::set_win_e_enabled(on) {
                        Ok(()) => app.win_e_enabled = on,
                        Err(error) => app.toast_error(
                            rust_i18n::t!("settings_win_e_error", error = error.to_string()).into_owned(),
                        ),
                    }
                },
                default: None,
            },
        },
    );
    list
});

impl App {
    /// Persist settings and snapshot the per-profile workspace knobs. The single
    /// write path for every settings mutation.
    pub(crate) fn commit_settings(&mut self) {
        self.publish_window_preferences = true;
        self.state.profiles.active_mut().workspace = self.state.settings.workspace();
        if self.state.profiles.active().prefs.contains(atlas_model::profiles::SETTINGS_PREFS_KEY) {
            self.state.profiles.active_mut().prefs.set(atlas_model::profiles::SETTINGS_PREFS_KEY, &self.state.settings);
        }
        atlas_core::util::config::save_profiles(&self.state.profiles);
        atlas_core::util::config::save_settings(&self.state.settings);
    }

    /// Close the settings overlay without needing a window (actions, links).
    /// Cancels any open picker so its live preview is reverted.
    pub(crate) fn close_settings_state(&mut self) {
        self.settings_move = None;
        self.settings_cancel_drill();
        self.cancel_command_shortcut_capture();
        self.commit_settings();
        self.state.overlay.show_settings = false;
        self.settings_input = None;
        self.settings_ui = SettingsUi::default();
    }

    /// Revert the open picker's live preview and leave it (no input reset).
    pub(crate) fn settings_cancel_drill(&mut self) {
        let Some(drill) = self.settings_ui.drill.take() else {
            return;
        };
        if let Some(Control::Choice { preview: Some(preview), .. }) = find(drill.setting).map(|d| &d.control) {
            preview(self, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_keys_resolve() {
        // rust-i18n's debug-build initialiser needs more than the 2 MB test stack.
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(check_registry)
            .unwrap()
            .join()
            .unwrap();
    }

    fn check_registry() {
        let mut ids: Vec<_> = SETTINGS.iter().map(|d| d.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate setting id");
        rust_i18n::set_locale("en");
        for def in SETTINGS.iter() {
            assert_ne!(def.title().as_ref(), def.title, "missing en title for {}", def.id);
            if let Some(key) = def.desc {
                assert_ne!(rust_i18n::t!(key).as_ref(), key, "missing en desc for {}", def.id);
            }
        }
        for category in Category::ALL {
            assert!(!category.title().starts_with("sp_"));
        }
    }

    #[test]
    fn slider_snaps_and_clamps() {
        let slider = Slider {
            min: 0.2,
            max: 1.0,
            step: 0.05,
            unit: Unit::Percent,
            get: |_| 0.0,
            set: |_, _| {},
            default: None,
            live: true,
        };
        assert!((slider.snap(0.47) - 0.45).abs() < 1e-4);
        assert!((slider.snap(3.0) - 1.0).abs() < 1e-4);
        assert!((slider.snap(-1.0) - 0.2).abs() < 1e-4);
        assert_eq!(slider.format(0.45), "45%");
        assert!((slider.fraction(0.6) - 0.5).abs() < 1e-4);
    }
}
