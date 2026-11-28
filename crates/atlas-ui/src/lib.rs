//! `atlas-ui`: the gpui application layer — the `App` entity, views, overlays,
//! input/actions and platform glue. Depends on `atlas-core` (backends) and
//! `atlas-model` (state); the binary (`src/main.rs`) is a thin launcher over
//! [`run`].
//!
// reason: gpui's actions! macro derives PartialEq (not Eq) on every action struct.
#![allow(clippy::derive_partial_eq_without_eq)]
// Panic-free stdio: std's print macros panic when the stream is a closed
// pipe, and a panic aborts the whole GUI process. These shadow the prelude
// macros for every module of this crate and drop write errors instead.
#[allow(unused_macros)]
macro_rules! println {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::writeln!(::std::io::stdout(), $($arg)*);
    }};
}
#[allow(unused_macros)]
macro_rules! eprintln {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::writeln!(::std::io::stderr(), $($arg)*);
    }};
}
#[allow(unused_macros)]
macro_rules! print {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::write!(::std::io::stdout(), $($arg)*);
    }};
}
#[allow(unused_macros)]
macro_rules! eprint {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::write!(::std::io::stderr(), $($arg)*);
    }};
}


rust_i18n::i18n!("locales", fallback = "en");

mod input;

pub mod app;
pub mod assets;
pub use atlas_model::build_info;
/// Platform drag-and-drop: neutral types + Windows OLE implementation.
pub(crate) mod dnd;
pub(crate) mod icon_theme;
pub(crate) mod perf;
/// Windows shell context-menu integration (IContextMenu enumeration + invoke).
#[cfg(target_os = "windows")]
pub(crate) mod shell_menu;
/// Windows Shell thumbnail cache access via IShellItemImageFactory.
#[cfg(target_os = "windows")]
pub(crate) mod shell_thumb;
#[cfg(windows)]
pub(crate) mod shell_file_operation;
pub mod ui;
pub(crate) mod ui_wake;

use gpui::{px, size, App as GpuiApp, AppContext as _, KeyBinding, WindowOptions};
use gpui_component::{Root, ThemeRegistry, TitleBar};
use std::time::Instant;

gpui::actions!(
    atlas,
    [
        Quit,
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        NavUp,
        NavDown,
        NavHome,
        NavEnd,
        NavPageUp,
        NavPageDown,
        ExtendUp,
        ExtendDown,
        OpenSelected,
        SelectAllItems,
        RenameSelected,
        // Phase D: overlays / panels / view toggles.
        OpenPalette,
        OpenGotoBar,
        OpenSearchInPlace,
        TogglePreviewExpanded,
        ToggleInspector,
        ToggleSidebar,
        ToggleHidden,
        OpenSettings,
        OpenExtManager,
        CloseOverlays,
        // Phase D: pane structure.
        CloseFocusedPane,
        FocusNextPane,
        SplitHorizontal,
        SplitVertical,
        // Phase D: history / location / refresh.
        Undo,
        Redo,
        RefreshTab,
        EditLocation,
        NavBack,
        NavForward,
        NavParent,
        DeleteToTrash,
        DeletePermanent,
        SidebarNewGlobalTag,
        SidebarNewProfileTag,
        // Bottom-bar view selector (per-pane dropdown).
        ViewModeDetails,
        ViewModeGrid,
        ViewModeList,
        ViewModeMiller,
        ZoomIn,
        ZoomOut,
        ZoomReset,
    ]
);

#[cfg(feature = "provider-host")]
#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct RemoveCloudRemote {
    pub id: String,
}

#[cfg(feature = "provider-host")]
#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct StartRenameCloudRemote {
    pub id: String,
}

#[cfg(all(feature = "provider-host", windows))]
#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct InstallWinFspForRemote {
    pub id: String,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct SidebarToggleSection {
    pub index: usize,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct SidebarHideSection {
    pub index: usize,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct SidebarMoveSectionUp {
    pub index: usize,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct SidebarMoveSectionDown {
    pub index: usize,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct SidebarResetSections;

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct OpenSpaceLensForPath {
    pub path: std::path::PathBuf,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct OpenPropertiesForPath {
    pub path: std::path::PathBuf,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct OpenThisPcProperties;

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct SidebarDeleteTag {
    pub tag: atlas_model::tag_cache::TagId,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct SidebarRenameTag {
    pub tag: atlas_model::tag_cache::TagId,
    pub name: String,
    pub color: [u8; 3],
    pub global: bool,
}

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = atlas, no_json)]
pub struct SidebarRemoveBookmark {
    pub index: usize,
}


/// Launch the gpui application. The caller owns the tokio runtime and passes its
/// handle in; gpui drives the UI on the calling (main) thread.
pub fn run(
    handle: tokio::runtime::Handle,
    app_started: Instant,
    on_quit: Option<Box<dyn FnOnce() + 'static>>,
    initial_paths: Vec<std::path::PathBuf>,
) {
    let run_started = Instant::now();
    #[cfg(feature = "ui-lab")]
    let initial_paths = crate::ui::preview_lab::prepare(initial_paths);
    let settings = atlas_core::util::config::load_settings();
    let lang = settings
        .language
        .clone()
        .unwrap_or_else(|| "en".to_string());
    rust_i18n::set_locale(&lang);
    // Build the translation backend now, on a shallow stack. Left lazy, its
    // large init frame lands at the bottom of the first window's layout
    // recursion and overflows the main thread.
    let _ = rust_i18n::available_locales!();
    tracing::info!(
        total_ms = app_started.elapsed().as_millis(),
        phase_ms = run_started.elapsed().as_millis(),
        "startup: settings+locale"
    );

    let application = gpui_platform::application().with_assets(crate::assets::AtlasAssets);
    application.run(move |cx: &mut GpuiApp| {
        cx.set_app_identity("digART.FLDR", "FLDR");
        let gpui_started = Instant::now();
        gpui_component::init(cx);
        app::windowing::install(cx);
        if let Err(err) = cx.text_system().add_fonts(crate::assets::bundled_fonts()) {
            tracing::warn!(?err, "failed to load bundled Geist fonts");
        }
        tracing::info!(
            total_ms = app_started.elapsed().as_millis(),
            phase_ms = gpui_started.elapsed().as_millis(),
            "startup: gpui init"
        );

        // Register the seven bundled FLDR themes and watch the user
        // themes dir for custom `ThemeSet` JSON files. The active profile's theme is
        // applied on first render (see `App::apply_active_theme`).
        let themes_started = Instant::now();
        let json = include_str!("../assets/themes/fldr-themes.json");
        match ThemeRegistry::global_mut(cx).load_themes_from_str(json) {
            Ok(()) => {
                eprintln!(
                    "[fldr] {} themes registered",
                    ThemeRegistry::global(cx).themes().len()
                );
            }
            Err(err) => {
                eprintln!("[fldr] FAILED to load bundled themes: {err:#}");
            }
        }
        // CRITICAL: Load bundled themes immediately in-memory so the active profile
        // theme is available on the first frame. Filesystem sync and directory watching
        // for custom user themes are deferred to the background so they do not block startup.
        let themes_dir = atlas_core::util::config::themes_dir();
        cx.spawn(async move |cx| {
            if let Err(err) = std::fs::create_dir_all(&themes_dir) {
                tracing::error!(?err, "failed to create themes directory");
            } else {
                // Built-in FLDR themes are embedded. Archive the previously
                // generated Atlas bundle so it cannot override local presets.
                let legacy = themes_dir.join("atlas-themes.json");
                if legacy.is_file() {
                    for index in 0..100 {
                        let backup = themes_dir.join(format!(
                            "atlas-themes.json.legacy-{index}.bak"
                        ));
                        if backup.exists() {
                            continue;
                        }
                        match std::fs::rename(&legacy, &backup) {
                            Ok(()) => break,
                            Err(err) => {
                                tracing::warn!(?err, "failed to archive legacy theme bundle");
                                break;
                            }
                        }
                    }
                }
            }
            // The watcher reloads themes from disk and replaces its registry.
            // Synchronize the embedded built-ins before starting the watcher.
            if let Err(err) = atlas_core::util::config::sync_bundled_themes(&themes_dir, json) {
                tracing::error!(?err, "failed to synchronize FLDR themes");
            }

            _ = cx.update(|cx| {
                if let Err(err) = ThemeRegistry::watch_dir(themes_dir, cx, |cx| {
                    cx.refresh_windows();
                }) {
                    tracing::error!(?err, "failed to watch themes dir");
                }
            });
        })
        .detach();

        tracing::info!(
            total_ms = app_started.elapsed().as_millis(),
            phase_ms = themes_started.elapsed().as_millis(),
            "startup: themes"
        );

        cx.bind_keys([
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-q", Quit, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("up", NavUp, Some("Atlas")),
            KeyBinding::new("down", NavDown, Some("Atlas")),
            KeyBinding::new("shift-up", ExtendUp, Some("Atlas")),
            KeyBinding::new("shift-down", ExtendDown, Some("Atlas")),
            KeyBinding::new("home", NavHome, Some("Atlas")),
            KeyBinding::new("end", NavEnd, Some("Atlas")),
            KeyBinding::new("pageup", NavPageUp, Some("Atlas")),
            KeyBinding::new("pagedown", NavPageDown, Some("Atlas")),
            KeyBinding::new("ctrl-tab", NextTab, Some("Atlas")),
            KeyBinding::new("ctrl-shift-tab", PrevTab, Some("Atlas")),
            // Phase D: overlays / panels / toggles.
            KeyBinding::new("ctrl-f", OpenSearchInPlace, Some("Atlas")),
            KeyBinding::new("space", ToggleInspector, Some("Atlas")),
            KeyBinding::new("escape", CloseOverlays, Some("Atlas")),
            // Phase D: pane structure.
            KeyBinding::new("alt-tab", FocusNextPane, Some("Atlas")),
            // Phase D: history / location / refresh.
            KeyBinding::new("ctrl-r", RefreshTab, Some("Atlas")),
            KeyBinding::new("ctrl-l", EditLocation, Some("Atlas")),
            KeyBinding::new("alt-up", NavParent, Some("Atlas")),
            // Open/rename/refresh and file clipboard keys resolve through the
            // profile command registry, so remapping removes the old binding.
            // Text input clipboard bindings remain owned by Input context.
        ]);
        cx.on_action(|_: &Quit, cx: &mut GpuiApp| {
            // Keep copied files available to other apps after Atlas exits.
            #[cfg(windows)]
            crate::dnd::windows::ole_clipboard::flush();
            cx.quit();
        });

        #[cfg(feature = "ui-lab")]
        let hidden_preview_lab = std::env::var_os("ATLAS_PREVIEW_LAB").is_some();
        #[cfg(not(feature = "ui-lab"))]
        let hidden_preview_lab = false;
        let mut sessions = if settings.autosave_session && initial_paths.is_empty() && !hidden_preview_lab {
            atlas_core::util::config::load_window_sessions().into_iter()
        } else {
            Vec::new().into_iter()
        };
        let first_session = sessions.next();
        let additional_sessions: Vec<_> = sessions.collect();
        let geometry = first_session.as_ref().and_then(|session| session.geometry.as_ref())
            .or(settings.window_geometry.as_ref());
        let (restored_bounds, restored_display) = ui::window_geometry::restore(geometry, cx);
        let window_options = WindowOptions {
            // Windows delivers demand-driven frames through WM_PAINT. A hidden
            // window may never receive it, so visibility cannot wait for a frame.
            show: !hidden_preview_lab,
            focus: !hidden_preview_lab,
            titlebar: Some(TitleBar::title_bar_options()),
            window_bounds: Some(restored_bounds),
            display_id: restored_display,
            window_min_size: Some(size(px(640.), px(400.))),
            app_id: Some("digART.FLDR".to_string()),
            ..Default::default()
        };

        let mut app_settings = settings.clone();
        app_settings.window_geometry = geometry.cloned();
        let window_started = Instant::now();
        if let Err(err) = cx.open_window(window_options, move |window, cx| {
            tracing::info!(
                total_ms = app_started.elapsed().as_millis(),
                phase_ms = window_started.elapsed().as_millis(),
                "startup: window callback"
            );
            window.set_window_title(&rust_i18n::t!("app_name"));
            let initial_paths = initial_paths.clone();
            let view = cx.new(|cx| {
                let seed = first_session.map(|session| app::windowing::WindowSeed {
                    session: Some(session),
                    legacy_session_writer: true,
                    ..Default::default()
                });
                app::App::new_seeded(handle, app_settings, app_started, initial_paths, seed, window, cx)
            });
            #[cfg(feature = "conversion-lab")]
            crate::ui::converter::lab::install(&view, window, cx);
            #[cfg(feature = "ui-lab")]
            crate::ui::preview_lab::install(&view, window, cx);
            app::App::schedule_startup_frames(window, 2);
            app::windowing::register(&view, window, cx);
            app::windowing::restore_secondary_windows(&view, additional_sessions, window, cx);
            // The top-level window view must be `Root` (mount point for
            // gpui-component overlays, modals and notifications).
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            tracing::error!(?err, "failed to open window");
        }
        tracing::info!(
            total_ms = app_started.elapsed().as_millis(),
            phase_ms = window_started.elapsed().as_millis(),
            "startup: open window"
        );

        if let Some(on_quit) = on_quit {
            let mut on_quit_opt = Some(on_quit);
            cx.on_app_quit(move |_| {
                let cb = on_quit_opt.take();
                async move {
                    if let Some(cb) = cb {
                        cb();
                    }
                }
            })
            .detach();
        }
    });
}
#[cfg(feature = "ui-lab")]
pub fn run_vault_diagnostic(directory: std::path::PathBuf) -> anyhow::Result<()> {
    app::vault::diagnostic::run(directory)
}

