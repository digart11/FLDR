//! PLACES edits only profile references, never filesystem objects.
use crate::app::App;
use atlas_model::places::{Place, PlaceEdit, PlaceTarget, Places, PlacesFeature};
use gpui::{Context, Entity, Window};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use std::path::PathBuf;

pub(crate) fn defaults() -> Vec<Place> {
    let mut entries: Vec<_> = [
        ("house", "sidebar_home", dirs::home_dir()),
        ("monitor", "sidebar_desktop", dirs::desktop_dir()),
        ("file-text", "sidebar_documents", dirs::document_dir()),
        ("download-simple", "sidebar_downloads", dirs::download_dir()),
        ("image", "sidebar_pictures", dirs::picture_dir()),
        ("music-notes", "sidebar_music", dirs::audio_dir()),
        ("film-strip", "sidebar_videos", dirs::video_dir()),
    ]
    .into_iter()
    .filter_map(|(icon, label, path)| {
        path.map(|path| Place {
            label: rust_i18n::t!(label).to_string(),
            target: PlaceTarget::Folder(path),
            icon: icon.into(),
            hidden: false,
        })
    })
    .collect();
    entries.push(Place {
        label: rust_i18n::t!("sidebar_recycle_bin").to_string(),
        target: PlaceTarget::RecycleBin,
        icon: "trash".into(),
        hidden: false,
    });
    entries
}

pub(crate) fn entries(app: &App) -> Vec<Place> {
    app.state
        .profiles
        .active()
        .scoped::<PlacesFeature>()
        .resolved(defaults())
}

pub(crate) fn rows(app: &App) -> Vec<(Place, bool)> {
    let saved = entries(app);
    #[cfg(windows)]
    let discovered = {
        let preferences = app.state.profiles.active().scoped::<atlas_model::places::SidebarFeature>();
        let mut seen: std::collections::HashSet<_> = saved.iter().filter_map(Place::key).collect();
        app.discovered_places.iter()
            .filter(|entry| entry.icon != "globe-simple" && entry.key().is_some_and(|key| {
                !preferences.hidden_provider_folders.contains(&key) && seen.insert(key)
            }))
            .cloned()
            .map(|entry| (entry, true))
            .collect::<Vec<_>>()
    };
    #[cfg(not(windows))]
    let discovered = Vec::new();
    saved.into_iter().map(|entry| (entry, false)).chain(discovered).collect()
}

pub(crate) fn icon(entry: &Place) -> &'static str {
    match entry.icon.as_str() {
        "house" => "house",
        "monitor" => "monitor",
        "file-text" => "file-text",
        "download-simple" => "download-simple",
        "image" => "image",
        "music-notes" => "music-notes",
        "film-strip" => "film-strip",
        "trash" => "trash",
        "cloud" => "cloud",
        "dropbox" => "dropbox",
        "seafile" => "seafile",
        "globe-simple" => "globe-simple",
        _ => "folder",
    }
}

impl App {
    #[cfg(windows)]
    pub(crate) fn refresh_discovered_places(&mut self, cx: &mut Context<Self>) {
        if self.places_refresh_pending || !std::mem::take(&mut self.places_refresh_requested) {
            return;
        }
        self.places_refresh_pending = true;
        let discovery = self.rt.spawn_blocking(atlas_core::fs::windows_places::discover);
        cx.spawn(async move |this, cx| {
            let result = discovery.await;
            let _ = this.update(cx, |this, cx| {
                this.places_refresh_pending = false;
                if let Ok(places) = result {
                    this.discovered_places = places;
                }
                cx.notify();
            });
        }).detach();
    }

    pub(crate) fn add_place(&mut self, path: PathBuf) {
        let label = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let mut places = self.state.profiles.active().scoped::<PlacesFeature>();
        if places.add(
            defaults(),
            Place {
                label,
                target: PlaceTarget::Folder(path),
                icon: "folder".into(),
                hidden: false,
            },
        ) {
            self.state
                .profiles
                .active_mut()
                .set_scoped::<PlacesFeature>(&places);
            self.save();
        }
    }

    fn edit_place(&mut self, key: &str, edit: PlaceEdit) {
        self.delete_owner = atlas_model::places::DeleteOwner::SidebarReference;
        let mut places = self.state.profiles.active().scoped::<PlacesFeature>();
        places.edit(defaults(), key, edit);
        self.state
            .profiles
            .active_mut()
            .set_scoped::<PlacesFeature>(&places);
        self.save();
    }
}

pub(crate) fn menu_for_entry(
    mut menu: PopupMenu,
    app: &Entity<App>,
    key: String,
    hidden: bool,
) -> PopupMenu {
    for (label, edit) in [
        ("Remove from Places", PlaceEdit::Remove),
        (
            if hidden {
                "Show in Places"
            } else {
                "Hide from Places"
            },
            PlaceEdit::ToggleHidden,
        ),
        ("Move Up", PlaceEdit::MoveUp),
        ("Move Down", PlaceEdit::MoveDown),
    ] {
        let app = app.clone();
        let key = key.clone();
        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
            app.update(cx, |app, cx| {
                app.edit_place(&key, edit);
                cx.notify();
            });
        }));
    }
    menu
}

pub(crate) fn customize_menu(
    mut menu: PopupMenu,
    app: &Entity<App>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    if let Some(path) = app
        .read(cx)
        .state
        .active_tab_data()
        .and_then(|tab| tab.location.fs_dir())
        .map(std::path::Path::to_path_buf)
    {
        let app = app.clone();
        menu = menu.item(PopupMenuItem::new("Add Current Folder to Places").on_click(
            move |_, _, cx| {
                app.update(cx, |app, cx| {
                    app.add_place(path.clone());
                    cx.notify();
                });
            },
        ));
    }
    for entry in entries(app.read(cx)) {
        let Some(key) = entry.key() else {
            continue;
        };
        let app = app.clone();
        let label = format!(
            "{}{}",
            entry.label,
            if entry.hidden { " (hidden)" } else { "" }
        );
        menu = menu.submenu(label, window, cx, move |menu, _, _| {
            menu_for_entry(menu, &app, key.clone(), entry.hidden)
        });
    }
    let app = app.clone();
    menu.separator().submenu(
        "Restore Default Places...",
        window,
        cx,
        move |menu, _, _| {
            let app = app.clone();
            menu.item(
                PopupMenuItem::new("Confirm: replace customized Places").on_click(
                    move |_, _, cx| {
                        app.update(cx, |app, cx| {
                            app.delete_owner = atlas_model::places::DeleteOwner::SidebarReference;
                            let mut places = Places::default();
                            places.restore(true);
                            app.state
                                .profiles
                                .active_mut()
                                .set_scoped::<PlacesFeature>(&places);
                            app.save();
                            cx.notify();
                        });
                    },
                ),
            )
        },
    )
}
