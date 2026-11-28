use crate::app::App;
use atlas_model::location::{network_server_root, Location};
use atlas_model::places::{NetworkLocation, SidebarFeature, SidebarPreferences};
use atlas_model::profiles::{SectionKind, SidebarSection};
use gpui::prelude::*;
use gpui::{
    div, AnyElement, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton, Window,
};
use gpui_component::input::InputState;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{h_flex, v_flex, ActiveTheme};
use std::collections::HashSet;
use std::path::{Component, Path, Prefix};

pub(crate) struct NetworkEditor {
    existing: Option<NetworkLocation>,
    label: Entity<InputState>,
    path: Entity<InputState>,
    error: Option<&'static str>,
}

impl NetworkEditor {
    pub(crate) fn focus(&self, window: &Window, cx: &gpui::App) -> FocusHandle {
        let label = self.label.read(cx).focus_handle(cx);
        if self.existing.is_some() || label.is_focused(window) {
            label
        } else {
            self.path.read(cx).focus_handle(cx)
        }
    }
}

pub(crate) fn sections(app: &App) -> Vec<(usize, SidebarSection)> {
    let profile = app.state.profiles.active();
    let preferences = profile.scoped::<SidebarFeature>();
    super::sidebar_presentation::sections(
        &profile.sidebar_sections,
        &preferences.visible_legacy_sections,
    )
}

fn is_network_target(path: &Path) -> bool {
    if network_server_root(path).is_some() {
        return true;
    }
    if !path.is_absolute() {
        return false;
    }
    if matches!(path.components().next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..)))
    {
        return true;
    }
    let path = atlas_model::places::normalized_path(&path.to_string_lossy());
    atlas_core::fs::mounts::list_drives()
        .iter()
        .filter(|drive| drive.is_network)
        .any(|drive| {
            let root = atlas_model::places::normalized_path(&drive.mount_point.to_string_lossy());
            root.zip(path.as_deref())
                .is_some_and(|(root, path)| path.starts_with(&root))
        })
}

/// Rows include saved references, Windows shortcuts and mapped volumes in that
/// order. Use the mapping's UNC target as its identity when Windows provides it.
pub(crate) fn references(app: &App) -> Vec<(NetworkLocation, bool)> {
    let mut prefs = app.state.profiles.active().scoped::<SidebarFeature>();
    let drives: Vec<_> = atlas_core::fs::mounts::list_drives()
        .into_iter()
        .filter(|drive| drive.is_network)
        .collect();
    let alias = |location: &NetworkLocation| {
        let key = location.key()?;
        for drive in &drives {
            let Some(root) =
                atlas_model::places::normalized_path(&drive.mount_point.to_string_lossy())
            else {
                continue;
            };
            let Some(share) =
                atlas_model::places::normalized_path(&drive.label).filter(|s| s.starts_with(r"\\"))
            else {
                continue;
            };
            if let Some(tail) = key.strip_prefix(&root) {
                return Some(if tail.is_empty() {
                    share
                } else {
                    format!(r"{}\{tail}", share.trim_end_matches('\\'))
                });
            }
        }
        Some(key)
    };
    let mut entries: Vec<_> = std::mem::take(&mut prefs.network_locations)
        .into_iter()
        .map(|location| (location, true))
        .collect();
    #[cfg(windows)]
    entries.extend(app.discovered_places.iter().filter_map(|place| {
        use atlas_model::places::PlaceTarget;
        let PlaceTarget::Folder(path) = &place.target else {
            return None;
        };
        if place.icon != "globe-simple"
            || place
                .key()
                .is_some_and(|key| prefs.hidden_network_shortcuts.contains(&key))
        {
            return None;
        }
        Some((
            NetworkLocation {
                name: place.label.clone(),
                path: path.clone(),
            },
            true,
        ))
    }));
    entries.extend(drives.iter().map(|drive| {
        let name = super::sidebar::drive_display_label(&drive.mount_point, &drive.label);
        (
            NetworkLocation {
                name,
                path: drive.mount_point.clone(),
            },
            false,
        )
    }));
    let mut seen = HashSet::new();
    entries.retain(|(location, _)| alias(location).is_some_and(|key| seen.insert(key)));
    entries
}

pub(crate) fn edit_menu(
    menu: PopupMenu,
    app: &Entity<App>,
    location: NetworkLocation,
) -> PopupMenu {
    let rename_app = app.clone();
    let remove_app = app.clone();
    let rename = location.clone();
    menu.item(
        PopupMenuItem::new("Rename location").on_click(move |_, window, cx| {
            rename_app.update(cx, |app, cx| {
                app.begin_network_editor(Some(rename.clone()), window, cx)
            });
        }),
    )
    .item(
        PopupMenuItem::new("Remove location").on_click(move |_, _, cx| {
            let Some(key) = location.key() else { return };
            remove_app.update(cx, |app, cx| {
                let mut prefs = app.state.profiles.active().scoped::<SidebarFeature>();
                prefs
                    .network_locations
                    .retain(|location| location.key().as_ref() != Some(&key));
                prefs.hidden_network_shortcuts.insert(key.clone());
                app.save_sidebar_preferences(&prefs);
                cx.notify();
            });
        }),
    )
}

pub(crate) fn customization_menu(
    mut menu: PopupMenu,
    app: &Entity<App>,
    cx: &gpui::App,
) -> PopupMenu {
    let profile = app.read(cx).state.profiles.active();
    let prefs = profile.scoped::<SidebarFeature>();
    for kind in [
        SectionKind::Places,
        SectionKind::Drives,
        SectionKind::Cloud,
        SectionKind::Recent,
        SectionKind::Bookmarks,
        SectionKind::Tags,
    ] {
        let shown = profile
            .sidebar_sections
            .iter()
            .find(|s| s.kind == kind)
            .is_some_and(|s| !s.hidden)
            && (!matches!(kind, SectionKind::Bookmarks | SectionKind::Tags)
                || prefs.visible_legacy_sections.contains(&kind));
        let title = format!("{} {}", if shown { "Hide" } else { "Show" }, kind.label());
        let app = app.clone();
        menu = menu.item(PopupMenuItem::new(title).on_click(move |_, _, cx| {
            app.update(cx, |app, cx| {
                let mut prefs = app.state.profiles.active().scoped::<SidebarFeature>();
                if matches!(kind, SectionKind::Bookmarks | SectionKind::Tags)
                    && !shown
                    && !prefs.visible_legacy_sections.contains(&kind)
                {
                    prefs.visible_legacy_sections.push(kind.clone());
                }
                let sections = &mut app.state.profiles.active_mut().sidebar_sections;
                if let Some(section) = sections.iter_mut().find(|s| s.kind == kind) {
                    section.hidden = shown;
                } else {
                    sections.push(SidebarSection::new(kind.clone()));
                }
                app.save_sidebar_preferences(&prefs);
                cx.notify();
            });
        }));
    }
    let dock_app = app.clone();
    menu = menu.item(
        PopupMenuItem::new(if prefs.hide_file_dock {
            "Show File Dock"
        } else {
            "Hide File Dock"
        })
        .on_click(move |_, _, cx| {
            dock_app.update(cx, |app, cx| {
                let mut prefs = app.state.profiles.active().scoped::<SidebarFeature>();
                prefs.hide_file_dock = !prefs.hide_file_dock;
                if prefs.hide_file_dock {
                    app.dropzone_bounds = None;
                    app.dropzone_rects.clear();
                    app.dropzone_scroll = None;
                }
                app.save_sidebar_preferences(&prefs);
                cx.notify();
            })
        }),
    );
    #[cfg(windows)]
    for entry in app
        .read(cx)
        .discovered_places
        .iter()
        .filter(|p| matches!(p.icon.as_str(), "dropbox" | "seafile"))
    {
        let Some(key) = entry.key() else { continue };
        let hidden = prefs.hidden_provider_folders.contains(&key);
        let title = format!("{} {}", if hidden { "Show" } else { "Hide" }, entry.label);
        let app = app.clone();
        menu = menu.item(PopupMenuItem::new(title).on_click(move |_, _, cx| {
            app.update(cx, |app, cx| {
                let mut prefs = app.state.profiles.active().scoped::<SidebarFeature>();
                if hidden {
                    prefs.hidden_provider_folders.remove(&key);
                } else {
                    prefs.hidden_provider_folders.insert(key.clone());
                }
                app.save_sidebar_preferences(&prefs);
                cx.notify();
            });
        }));
    }
    if !prefs.hidden_provider_folders.is_empty() {
        let app = app.clone();
        menu = menu.item(PopupMenuItem::new("Restore hidden cloud folders").on_click(
            move |_, _, cx| {
                app.update(cx, |app, cx| {
                    let mut prefs = app.state.profiles.active().scoped::<SidebarFeature>();
                    prefs.hidden_provider_folders.clear();
                    app.save_sidebar_preferences(&prefs);
                    cx.notify();
                });
            },
        ));
    }
    menu
}

impl App {
    fn save_sidebar_preferences(&mut self, prefs: &SidebarPreferences) {
        self.state
            .profiles
            .active_mut()
            .set_scoped::<SidebarFeature>(prefs);
        self.save();
    }

    pub(crate) fn bookmark_network_locations(&mut self, paths: Vec<std::path::PathBuf>) {
        let mut prefs = self.state.profiles.active().scoped::<SidebarFeature>();
        for path in paths {
            if !atlas_model::location::is_network_root(&path) { continue; }
            let location = NetworkLocation { name: path.display().to_string(), path };
            let Some(key) = location.key() else { continue; };
            if !prefs.network_locations.iter().any(|entry| entry.key().as_ref() == Some(&key)) {
                prefs.network_locations.push(location);
            }
            prefs.hidden_network_shortcuts.remove(&key);
        }
        self.save_sidebar_preferences(&prefs);
    }

    pub(crate) fn begin_network_editor(
        &mut self,
        existing: Option<NetworkLocation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = existing
            .as_ref()
            .map(|location| location.path.clone())
            .or_else(|| {
                self.state
                    .active_tab_data()
                    .and_then(|tab| match &tab.location {
                        Location::NetworkServer(_) | Location::Directory(_) => {
                            Some(tab.location.display_path())
                        }
                        _ => None,
                    })
                    .filter(|path| is_network_target(path))
            });
        let label = existing
            .as_ref()
            .map_or("", |location| location.name.as_str());
        self.network_editor = Some(NetworkEditor {
            label: crate::ui::text_input::single_line(window, cx, "Name", label),
            path: crate::ui::text_input::single_line(
                window,
                cx,
                r"\\server\share",
                path.map_or_else(String::new, |p| p.display().to_string()),
            ),
            existing,
            error: None,
        });
        cx.notify();
    }

    fn submit_network_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.network_editor.as_mut() else {
            return;
        };
        let name = editor.label.read(cx).value().trim().to_owned();
        let path = if let Some(existing) = &editor.existing {
            existing.path.clone()
        } else {
            let text = editor.path.read(cx).value();
            match super::navigation_policy::edited_path(&text, Path::new("")) {
                Ok(path) if is_network_target(&path) => path,
                _ => {
                    editor.error = Some("Enter a UNC server, share or mapped network path");
                    return;
                }
            }
        };
        let name = if name.is_empty() {
            path.display().to_string()
        } else {
            name
        };
        let location = NetworkLocation { name, path };
        let Some(key) = location.key() else {
            editor.error = Some("Invalid network path");
            return;
        };
        let mut prefs = self.state.profiles.active().scoped::<SidebarFeature>();
        if let Some(saved) = prefs
            .network_locations
            .iter_mut()
            .find(|saved| saved.key().as_ref() == Some(&key))
        {
            *saved = location;
        } else {
            prefs.network_locations.push(location);
        }
        prefs.hidden_network_shortcuts.remove(&key);
        self.save_sidebar_preferences(&prefs);
        self.network_editor = None;
    }

    pub(crate) fn render_network_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.overlay_focus(cx);
        let Some(editor) = self.network_editor.as_ref() else {
            return div().into_any_element();
        };
        let title = if editor.existing.is_some() {
            "Rename network location"
        } else {
            "Add network location"
        };
        let theme = cx.theme();
        let (popover, fg, border, accent, accent_fg, danger) = (
            theme.popover,
            theme.popover_foreground,
            theme.border,
            theme.accent,
            theme.accent_foreground,
            theme.danger,
        );
        let panel = v_flex()
            .w(gpui::px(360.))
            .max_w_full()
            .p_4()
            .gap_3()
            .rounded_lg()
            .occlude()
            .bg(popover)
            .text_color(fg)
            .border_1()
            .border_color(border)
            .track_focus(&focus)
            .capture_key_down(cx.listener(|app, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => app.network_editor = None,
                    "enter" => app.submit_network_editor(cx),
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
            .when(editor.existing.is_none(), |panel| {
                panel.child(crate::ui::text_input::bordered(&editor.path))
            })
            .child(crate::ui::text_input::bordered(&editor.label))
            .when_some(editor.error, |panel, error| {
                panel.child(div().text_color(danger).child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        div()
                            .id("network-editor-cancel")
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .child("Cancel")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|app, _, _, cx| {
                                    app.network_editor = None;
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("network-editor-save")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(accent)
                            .text_color(accent_fg)
                            .cursor_pointer()
                            .child("Save")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|app, _, _, cx| {
                                    app.submit_network_editor(cx);
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        self.overlay_scrim_custom(
            "scrim-network-editor",
            cx.entity(),
            |app| app.network_editor = None,
            focus,
            panel.into_any_element(),
            cx,
        )
    }
}
