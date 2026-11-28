//! Execute the UI's pure routing/layout policies without linking the GPUI renderer.
#![allow(dead_code)]
#[path = "../../atlas-ui/src/input/commands.rs"]
mod commands;
#[path = "../../atlas-ui/src/ui/chrome_geometry.rs"]
mod geometry;
#[path = "../../atlas-ui/src/ui/navigation_policy.rs"]
mod navigation;
#[path = "../../atlas-ui/src/ui/tab_overflow.rs"]
mod overflow;
#[path = "../../atlas-ui/src/ui/path_completion.rs"]
mod completion;
#[path = "../../atlas-ui/src/ui/tab_shape.rs"]
mod tab_shape;
#[path = "../../atlas-ui/src/ui/sidebar_presentation.rs"]
mod sidebar_presentation;
#[path = "../../atlas-ui/src/ui/address_policy.rs"]
mod address_policy;

#[test]
fn completion_enumerates_only_immediate_matching_directories() {
    use std::sync::atomic::AtomicU64;
    let fixture = tempfile::tempdir().unwrap();
    for name in ["Dropbox", "Drop space", "Dröp Unicode", "Other"] {
        std::fs::create_dir(fixture.path().join(name)).unwrap();
    }
    std::fs::create_dir(fixture.path().join("Other").join("Drop nested")).unwrap();
    std::fs::write(fixture.path().join("Drop file"), b"x").unwrap();
    let generation = AtomicU64::new(7);
    let run = |text| completion::enumerate(completion::query(text, fixture.path()).unwrap(), &generation, 7);
    assert_eq!(run("Drop"), vec!["Drop space\\", "Dropbox\\"]);
    assert_eq!(run("Drö"), vec!["Dröp Unicode\\"]);
    assert_eq!(run("\"Drop"), vec!["\"Drop space\\", "\"Dropbox\\"]);
    assert_eq!(run("\"Drop\""), vec!["\"Drop space\\\"", "\"Dropbox\\\""]);
    assert!(completion::enumerate(completion::query("Drop", fixture.path()).unwrap(), &generation, 6).is_empty());
    assert!(run("Other\\missing\\").is_empty());
}

#[test]
fn completion_parsing_and_ghost_preserve_windows_text() {
    let base = Path::new(r"C:\base");
    for (text, parent) in [(r"D:\Drop", r"D:\"), (r"\\server\share\Drop", r"\\server\share\"),
        (r"\\?\C:\Drop", r"\\?\C:\"), (r"\\?\UNC\server\share\Drop", r"\\?\UNC\server\share\"),
        ("\"D:\\Drop space", "D:\\")] {
        assert_eq!(completion::query(text, base).unwrap().parent, PathBuf::from(parent));
    }
    for text in ["", r"D:folder", r"D:folder\child", r"\\server", r"\\server\share", "//server", "//server/share",
        r"\\?\UNC\server\", "https://example.org", "bad\npath"] {
        assert!(completion::query(text, base).is_none(), "{text}");
    }
    assert_eq!(completion::suffix(r"D:\Drop", r"D:\Dropbox\"), Some("box\\"));
    assert_eq!(completion::suffix("Drö", "Dröp\\"), Some("p\\"));
    assert_eq!(completion::suffix("drop", "Dropbox\\"), Some("box\\"));
    assert_eq!(completion::suffix("other", "Dropbox\\"), None);
    assert_eq!(completion::suffix("\"Drop\"", "\"Dropbox\\\""), None);
}

#[test]
fn completion_cap_is_sorted_and_repeatable() {
    use std::sync::atomic::AtomicU64;
    let fixture = tempfile::tempdir().unwrap();
    for index in (0..20).rev() { std::fs::create_dir(fixture.path().join(format!("Dir {index:02}"))).unwrap(); }
    let run = || completion::enumerate(completion::query("Dir", fixture.path()).unwrap(), &AtomicU64::new(1), 1);
    let expected: Vec<_> = (0..8).map(|i| format!("Dir {i:02}\\")).collect();
    assert_eq!(run(), expected); assert_eq!(run(), expected);
}

use atlas_model::{location::Location, settings::ViewMode};
use std::path::{Path, PathBuf};

#[tokio::test]
async fn parent_control_does_not_enter_the_enumeration_or_selected_paths() {
    use atlas_model::{
        file_entry::{SortColumn, SortDir},
        pane::FolderOrder,
    };
    let fixture = tempfile::tempdir().unwrap();
    std::fs::create_dir(fixture.path().join("folder")).unwrap();
    std::fs::write(fixture.path().join("file.txt"), b"data").unwrap();
    let entries = atlas_core::fs::list::list_dir(
        fixture.path(),
        true,
        SortColumn::Name,
        SortDir::Asc,
        FolderOrder::FoldersFirst,
    )
    .await
    .unwrap();
    let paths: Vec<_> = entries.iter().map(|e| e.path().to_path_buf()).collect();
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().all(|p| p.parent() == Some(fixture.path())));
    assert!(navigation::parent_path(
        &Location::Directory(fixture.path().into()),
        &ViewMode::Details
    )
    .is_some());
    // Enumeration, hence select-all / transfers / count consumers, remains real-only.
    assert!(!paths.contains(&fixture.path().parent().unwrap().to_path_buf()));
}

#[test]
fn parent_entry_is_navigation_for_all_three_file_views() {
    for mode in [ViewMode::Details, ViewMode::List, ViewMode::Grid] {
        assert_eq!(
            navigation::parent_path(&Location::Directory(PathBuf::from(r"D:\work\child")), &mode),
            Some(PathBuf::from(r"D:\work"))
        );
    }
}

#[test]
fn roots_and_virtual_locations_have_no_parent_entry() {
    for path in [
        r"C:\",
        r"D:\",
        r"\\server\share\",
        r"\\?\C:\",
        r"\\?\UNC\server\share\",
    ] {
        assert_eq!(
            navigation::parent_path(&Location::Directory(path.into()), &ViewMode::Details),
            None,
            "{path}"
        );
    }
    assert_eq!(
        navigation::parent_path(&Location::RecycleBin, &ViewMode::Grid),
        None
    );
    assert_eq!(
        navigation::parent_path(
            &Location::Archive {
                archive: r"C:\test.zip".into(),
                inner: "sub".into()
            },
            &ViewMode::List
        ),
        None
    );
    for mode in [ViewMode::Miller, ViewMode::Tree] {
        assert_eq!(
            navigation::parent_path(&Location::Directory(r"C:\work".into()), &mode),
            None
        );
    }
}

#[test]
fn parent_enter_routes_to_navigation_and_arrows_return_to_file_navigation() {
    use navigation::{parent_key, ParentKey::*};
    assert_eq!(parent_key("enter", false, false, false), Open);
    assert_eq!(parent_key("enter", true, false, false), Other);
    assert_eq!(parent_key("enter", false, true, false), Other);
    assert_eq!(parent_key("escape", false, false, false), Leave);
    for key in [
        "up", "down", "left", "right", "home", "end", "pageup", "pagedown",
    ] {
        assert_eq!(parent_key(key, false, false, false), LeaveAndForward);
    }
    for key in ["delete", "c", "x", "v", "f2"] {
        assert_eq!(parent_key(key, true, false, false), Other);
    }
}

#[test]
fn only_exact_address_chords_trigger_editing() {
    assert!(navigation::is_address_shortcut(
        "l", true, false, false, false
    ));
    assert!(navigation::is_address_shortcut(
        "d", false, true, false, false
    ));
    for (key, ctrl, alt, shift, platform) in [
        ("l", false, false, false, false),
        ("d", true, false, false, false),
        ("l", true, true, false, false),
        ("l", true, false, true, false),
        ("d", false, true, false, true),
        ("k", true, false, false, false),
    ] {
        assert!(!navigation::is_address_shortcut(
            key, ctrl, alt, shift, platform
        ));
    }
}

#[test]
fn search_binding_is_preserved_and_split_shortcut_uses_toggle() {
    assert_eq!(
        commands::default_command_shortcut("nav.goto"),
        Some("ctrl-k")
    );
    assert_eq!(
        commands::default_command_shortcut("pane.toggle-split"),
        Some("ctrl-shift-s")
    );
    assert!(!commands::COMMANDS
        .iter()
        .any(|c| c.default_shortcut == Some("alt-d")));
    // Keep the unrelated experimental shortcut unassigned.
    assert!(!commands::COMMANDS
        .iter()
        .any(|c| c.default_shortcut == Some("ctrl-alt-v")));
    let splits: Vec<_> = commands::COMMANDS
        .iter()
        .filter_map(|c| {
            c.default_shortcut
                .filter(|k| *k == "ctrl-shift-s" || *k == "ctrl-alt-s")
                .map(|k| (c.id, k))
        })
        .collect();
    assert_eq!(splits.len(), 2);
    assert_eq!(splits, [("pane.toggle-split", "ctrl-shift-s"), ("pane.split-v", "ctrl-alt-s")]);
}

#[test]
fn pasted_windows_unc_and_unicode_paths_are_not_search_queries() {
    let base = Path::new(r"D:\work");
    for text in [
        r"C:\Users\Lucas",
        r"\\server\share\設計\café",
        r"D:\資料\emoji-📁",
        r"\\?\C:\long path",
    ] {
        assert_eq!(navigation::edited_path(text, base), Ok(PathBuf::from(text)));
    }
    assert_eq!(
        navigation::edited_path(" \"D:\\space here\\資料\" ", base),
        Ok(PathBuf::from(r"D:\space here\資料"))
    );
}

#[test]
fn drive_relative_and_parent_paths_are_resolved_without_filesystem_io() {
    let base = Path::new(r"D:\work\child");
    assert_eq!(
        navigation::edited_path("C:", base),
        Ok(PathBuf::from(r"C:\"))
    );
    assert_eq!(
        navigation::edited_path(r"..\sibling", base),
        Ok(PathBuf::from(r"D:\work\sibling"))
    );
    assert_eq!(
        navigation::edited_path(r"\資料", base),
        Ok(PathBuf::from(r"D:\資料"))
    );
    assert_eq!(
        navigation::edited_path(r"C:\..\..\data", base),
        Ok(PathBuf::from(r"C:\data"))
    );
    assert!(navigation::edited_path("C:ambiguous", base).is_err());
}

#[test]
fn invalid_pastes_stay_in_editor_instead_of_opening_content_search() {
    for text in [
        "",
        "  ",
        "\"\"",
        "a\nb",
        "C:\\a\0b",
        "https://example.com",
        "\"unclosed",
    ] {
        assert!(
            navigation::edited_path(text, Path::new(r"D:\work")).is_err(),
            "{text:?}"
        );
    }
}

#[test]
fn caption_region_is_reserved_at_zoom_and_physical_scale() {
    for width in [240., 480., 800., 1920.] {
        for zoom in [0.75, 1., 1.25, 1.5, 2.] {
            for scale in [1., 1.25, 1.5, 2., 2.5] {
                for (row, inset) in [(46., 3.), (46., 6.), (40., 6.)] {
                    let top = geometry::caption_top(row * zoom, inset, zoom);
                    let safe = geometry::available_width(
                        0.,
                        inset,
                        width,
                        row * zoom,
                        width,
                        top,
                        zoom,
                        scale,
                    );
                    let controls_left =
                        width - (geometry::CONTROL_INSET + geometry::CONTROL_WIDTH) * zoom;
                    assert!(safe >= 0. && safe <= width);
                    assert!(
                        safe <= (controls_left - geometry::CONTROL_GAP * zoom).max(0.) + 0.0001
                    );
                    assert!((safe * scale - (safe * scale).round()).abs() < 0.001);
                }
            }
        }
    }
}

#[test]
fn other_split_panes_do_not_lose_caption_space() {
    let top = geometry::caption_top(46., 3., 1.);
    assert_eq!(
        geometry::available_width(0., 100., 800., 46., 800., top, 1., 1.5),
        800.
    );
    assert_eq!(
        geometry::available_width(0., 3., 300., 46., 800., top, 1., 1.5),
        300.
    );
    assert_eq!(
        geometry::available_width(700., 3., 100., 46., 800., top, 1., 1.5),
        0.
    );
}

#[test]
fn caption_reservation_tracks_resize_and_zoom_instead_of_old_coordinates() {
    let first = geometry::available_width(
        0.,
        3.,
        800.,
        46.,
        800.,
        geometry::caption_top(46., 3., 1.),
        1.,
        1.,
    );
    let zoomed = geometry::available_width(
        0.,
        3.,
        800.,
        92.,
        800.,
        geometry::caption_top(92., 3., 2.),
        2.,
        1.5,
    );
    assert!(zoomed < first);
    let resized = geometry::available_width(
        0.,
        3.,
        400.,
        92.,
        400.,
        geometry::caption_top(92., 3., 2.),
        2.,
        1.5,
    );
    assert!(resized < zoomed);
}

#[test]
fn narrow_strip_controls_cannot_steal_tab_hit_area() {
    for zoom in [0.75, 1., 1.25, 1.5, 2.] {
        for width in 0..800 {
            let width = width as f32;
            let button = 32. * zoom;
            let gap = 2. * zoom;
            let pad = 8. * zoom;
            let add = overflow::add_button_fits(width, pad, button, gap);
            let available = (width - 2. * pad - if add { button + gap } else { 0. }).max(0.);
            if overflow::pagers_fit(available, button, gap, zoom) {
                assert!(add);
                assert!(available - 2. * (button + gap) >= 48. * zoom);
            }
            if add {
                let right = pad + available;
                let x = overflow::new_tab_x(right, gap, width - pad - button);
                assert!(x >= right);
                assert!(x + button <= width - pad + 0.001);
            }
        }
    }
}
