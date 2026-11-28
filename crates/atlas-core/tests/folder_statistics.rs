use atlas_model::{
    file_entry::{FileEntry, FileEntryData, FileKind, SortColumn, SortDir},
    folder_stats::{Cache, Inbox, SizeCell, MAX_AGE},
    pane::FolderOrder,
    tab::TabState,
};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Mutex},
    time::Instant,
};

// Existing manual dispatch/cancel contract, exercised without linking GPUI.
#[path = "../../atlas-ui/src/app/context_info/size_job.rs"]
mod size_job;

#[test]
fn automatic_viewport_includes_root_children_but_not_the_root_or_offscreen_rows() {
    use atlas_model::automatic_stats::visible_folders;
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let entries: Vec<_> = (0..300)
        .map(|n| entry(root.join(format!("dir-{n}")), true, 0))
        .collect();
    assert_eq!(
        visible_folders(root, &entries, &[8, 9]),
        [entries[8].path(), entries[9].path()]
    );
    assert_eq!(
        visible_folders(root, &entries, &(0..300).collect::<Vec<_>>()).len(),
        128
    );
    assert!(visible_folders(root, &entries, &[]).is_empty());
    assert!(visible_folders(root, &entries, &[500]).is_empty());
    let drive = root.ancestors().last().unwrap();
    assert!(visible_folders(drive, &entries, &[0, 1]).is_empty());
    let drive_children = [
        entry(drive.join("fileop-visible-child"), true, 0),
        entry(drive.join("fileop-offscreen-child"), true, 0),
    ];
    assert_eq!(
        visible_folders(drive, &drive_children, &[0]),
        [drive_children[0].path()]
    );
    assert_eq!(
        visible_folders(drive, &drive_children, &[0, 0]),
        [drive_children[0].path()]
    );
    let elsewhere = root.join("other");
    assert!(visible_folders(&elsewhere, &entries, &[0]).is_empty());
    let file = entry(root.join("file"), false, 30);
    assert!(visible_folders(root, &[file], &[0]).is_empty());
}

#[test]
fn viewport_and_cache_reads_remain_available_during_an_active_scan() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_path_buf();
    let entries = [entry(root.join("child"), true, 0)];
    let mut cache = Cache::default();
    let ticket = cache.begin(&[root.clone()], Instant::now()).unwrap();
    let start = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(
            atlas_model::automatic_stats::visible_folders(&root, &entries, &[0]).len(),
            1
        );
        assert_eq!(cache.cell(&root, Instant::now()), SizeCell::Calculating);
        assert_eq!(
            cache.cell(&entries[0].path(), Instant::now()),
            SizeCell::Unknown
        );
    }
    eprintln!(
        "10,000 in-memory viewport/cache reads with a reserved scanner: {:?}",
        start.elapsed()
    );
    // Reads have neither completed nor replaced the running reservation.
    assert!(cache.begin(&[entries[0].path()], Instant::now()).is_none());
    cache.finish_missing(ticket, true, Instant::now());
}

#[test]
fn obsolete_automatic_root_can_retry_without_discarding_completed_children() {
    use atlas_model::{folder_size::SizeReport, folder_stats::DirectoryStats};
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_path_buf();
    let child = root.join("child");
    let mut cache = Cache::default();
    let ticket = cache.begin(&[root.clone()], Instant::now()).unwrap();
    cache.publish(
        ticket,
        DirectoryStats::new(
            child.clone(),
            SizeReport {
                done: true,
                bytes: 99,
                files: 2,
                ..Default::default()
            },
        ),
        Instant::now(),
    );
    assert!(cache.begin(&[child.clone()], Instant::now()).is_none());
    cache.finish_missing(ticket, true, Instant::now());
    cache.retry_cancelled(&root);
    assert_eq!(cache.cell(&root, Instant::now()), SizeCell::Unknown);
    assert_eq!(cache.cell(&child, Instant::now()), SizeCell::Exact(99));
    cache.retry_cancelled(&child);
    assert_eq!(cache.cell(&child, Instant::now()), SizeCell::Exact(99));
    assert!(cache.begin(&[root], Instant::now()).is_some());
}

#[cfg(windows)]
#[test]
fn actual_windows_storage_query_and_unsupported_path() {
    use atlas_model::{
        automatic_stats::{enabled, StorageKind},
        settings::Settings,
    };
    let fixture = tempfile::tempdir().unwrap();
    let kind = atlas_win_space::storage::statistics_storage(fixture.path());
    eprintln!(
        "Windows temporary fixture storage: {kind:?}; seek penalty query: {:?}",
        atlas_win_space::storage::incurs_seek_penalty(fixture.path())
    );
    // No assertion that a local drive is an SSD: devices may be virtual/unknown.
    assert_eq!(
        enabled(&Settings::default(), kind),
        kind == StorageKind::Ssd
    );
    assert_eq!(
        atlas_win_space::storage::statistics_storage(std::path::Path::new(r"?:\unsupported")),
        StorageKind::Unknown
    );
    assert_eq!(
        atlas_win_space::storage::statistics_storage(fixture.path().ancestors().last().unwrap()),
        StorageKind::Unknown
    );
}

// Exercise the actual UI cell presenter without starting GPUI or the application.
#[path = "../../atlas-ui/src/ui/folder_stats_presentation.rs"]
mod presentation;

#[test]
fn existing_size_cell_formats_complete_progress_partial_and_unavailable() {
    assert_eq!(presentation::text(SizeCell::Exact(4831838208)), "4.5 GB");
    assert_eq!(presentation::text(SizeCell::Exact(0)), "0 B");
    assert_eq!(presentation::text(SizeCell::Cached(4831838208)), "4.5 GB");
    assert!(presentation::subdued(SizeCell::Cached(0)));
    assert!(!presentation::subdued(SizeCell::Exact(0)));
    assert!(!presentation::subdued(SizeCell::Stale));
    assert_eq!(presentation::text(SizeCell::Calculating), "…");
    assert_eq!(presentation::text(SizeCell::Partial(5120)), "≥5 KB");
    for cell in [
        SizeCell::Unknown,
        SizeCell::Failed,
        SizeCell::Cancelled,
        SizeCell::Stale,
    ] {
        assert_eq!(presentation::text(cell), "—");
    }
}

#[test]
fn regular_file_sort_contract_is_unchanged_in_all_folder_orders() {
    for order in FolderOrder::ALL {
        for direction in [SortDir::Asc, SortDir::Desc] {
            let entries = vec![
                entry("z".into(), false, 9),
                entry("a".into(), false, 3),
                entry("m".into(), false, 9),
            ];
            let mut original = entries.clone();
            let mut adapted = entries;
            atlas_core::fs::list::sort_entries(&mut original, &SortColumn::Size, &direction, order);
            atlas_core::fs::list::sort_entries_with_sizes(
                &mut adapted,
                &SortColumn::Size,
                &direction,
                order,
                |entry| entry.size(),
            );
            assert_eq!(
                original.iter().map(|e| e.path()).collect::<Vec<_>>(),
                adapted.iter().map(|e| e.path()).collect::<Vec<_>>()
            );
        }
    }
}

fn entry(path: PathBuf, directory: bool, bytes: u64) -> FileEntry {
    FileEntryData {
        name: path.file_name().unwrap().to_string_lossy().as_ref().into(),
        path,
        kind: if directory {
            FileKind::Dir
        } else {
            FileKind::File
        },
        size: if directory { None } else { Some(bytes) },
        modified: None,
        _is_hidden: false,
        _is_symlink: false,
        archive_root: None,
        recycle_info: None,
    }
    .into()
}

#[test]
fn real_worker_cache_cell_sort_and_invalidation_keep_file_metadata_and_selection() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::create_dir(root.join("large")).unwrap();
    std::fs::create_dir(root.join("small")).unwrap();
    std::fs::write(root.join("large/data"), [0; 5120]).unwrap();
    std::fs::write(root.join("small/data"), [0; 7]).unwrap();
    let paths = vec![root.join("large"), root.join("small")];
    let mut cache = Cache::default();
    let ticket = cache.begin(&paths, Instant::now()).unwrap();
    let inbox = Mutex::new(Inbox::new(&paths));
    let report = atlas_core::fs::folder_size::scan_with_stats(
        paths.clone(),
        &AtomicBool::new(false),
        |_| false,
        |_| {},
        |stats| inbox.lock().unwrap().push(stats),
    );
    assert_eq!((report.bytes, report.files, report.folders), (5127, 2, 0));
    for stats in inbox.lock().unwrap().drain() {
        cache.publish(ticket, stats, Instant::now());
    }
    cache.finish_missing(ticket, false, Instant::now());
    assert_eq!(
        presentation::text(cache.cell(&paths[0], Instant::now())),
        "5 KB"
    );
    assert_eq!(
        presentation::text(cache.cell(&paths[1], Instant::now())),
        "7 B"
    );
    let file = root.join("regular");
    let entries = vec![
        entry(paths[0].clone(), true, 0),
        entry(paths[1].clone(), true, 0),
        entry(file.clone(), false, 9),
    ];
    let mut state = TabState::default();
    state.set_entries(entries.into());
    state.selection.selected.insert(0);
    let selected = state.entries[0].path();
    state.mutate_preserving_selection(false, |state| {
        let mut entries = state.entries.to_vec();
        atlas_core::fs::list::sort_entries_with_sizes(
            &mut entries,
            &SortColumn::Size,
            &SortDir::Asc,
            FolderOrder::FoldersFirst,
            |entry| {
                if entry.kind() == FileKind::Dir {
                    cache.exact_bytes(&entry.path(), Instant::now())
                } else {
                    entry.size()
                }
            },
        );
        state.set_entries(entries.into());
    });
    assert_eq!(
        state.entries.iter().map(|e| e.path()).collect::<Vec<_>>(),
        [paths[1].clone(), paths[0].clone(), file]
    );
    assert_eq!(
        state.entries[*state.selection.selected.iter().next().unwrap()].path(),
        selected
    );
    assert!(state
        .entries
        .iter()
        .filter(|e| e.kind() == FileKind::Dir)
        .all(|e| e.size().is_none()));
    assert_eq!(state.entries[2].size(), Some(9)); // Bottom counts never receive recursive bytes.
    cache.invalidate(Some(&root.join("large/data")));
    assert_eq!(
        presentation::text(cache.cell(&paths[0], Instant::now())),
        "—"
    );
    assert_eq!(cache.cell(&paths[1], Instant::now()), SizeCell::Exact(7));
    cache.expire(Instant::now() + MAX_AGE);
    assert_eq!(
        presentation::text(cache.cell(&paths[1], Instant::now() + MAX_AGE)),
        "7 B"
    );
}

#[test]
fn denied_policy_and_missing_root_never_produce_exact_zero() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::create_dir(root.join("private")).unwrap();
    std::fs::write(root.join("private/data"), [0; 19]).unwrap();
    std::fs::write(root.join("visible"), [0; 3]).unwrap();
    let mut results = Vec::new();
    let report = atlas_core::fs::folder_size::scan_with_stats(
        vec![root.to_path_buf()],
        &AtomicBool::new(false),
        |path| path.file_name().is_some_and(|name| name == "private"),
        |_| {},
        |stats| results.push(stats),
    );
    assert_eq!(report.bytes, 3);
    assert_eq!(report.skipped, 1);
    let mut cache = Cache::default();
    let ticket = cache.begin(&[root.to_path_buf()], Instant::now()).unwrap();
    for stats in results {
        cache.publish(ticket, stats, Instant::now());
    }
    cache.finish_missing(ticket, false, Instant::now());
    assert_eq!(
        presentation::text(cache.cell(root, Instant::now())),
        "≥3 B"
    );
    let missing = root.join("not-present");
    let ticket = cache.begin(&[missing.clone()], Instant::now()).unwrap();
    atlas_core::fs::folder_size::scan_with_stats(
        vec![missing.clone()],
        &AtomicBool::new(false),
        |_| false,
        |_| {},
        |stats| {
            cache.publish(ticket, stats, Instant::now());
        },
    );
    cache.finish_missing(ticket, false, Instant::now());
    assert_eq!(cache.cell(&missing, Instant::now()), SizeCell::Failed);
    assert_eq!(
        presentation::text(cache.cell(&missing, Instant::now())),
        "—"
    );
}

// The actual persisted column policy is independent of GPUI rendering.
#[path = "../../atlas-ui/src/app/details_columns.rs"]
mod columns;

#[path = "../../atlas-ui/src/ui/view_selector_policy.rs"]
mod selector_policy;
#[path = "../../atlas-ui/src/ui/pointer_state.rs"]
mod selector_capture;

#[test]
fn group441_missing_legacy_columns_are_visible_even_with_reference_width() {
    use columns::{DetailsColumn as C, DetailsColumnPrefs as P};
    let saved = r#"{"widths":{"name":302.7266,"size":117.6138,"modified":119.6595,"type_w":110,"reference_width":650},"order":["Size","Modified","Type"]}"#;
    let prefs: P = serde_json::from_str(saved).unwrap();
    assert_eq!(prefs.widths.name, 302.7266);
    assert_eq!(prefs.widths.reference_width, Some(650.));
    for width in [650., 1000., 1600.] {
        let fitted = columns::responsive_widths(width, 1., prefs.widths, &prefs.order);
        assert!(fitted[4] >= 46. && fitted[5] >= 46.);
    }
    assert_eq!(&prefs.order[..3], &[C::Size, C::Modified, C::Type]);
    let round = P::from_folder(&prefs.to_folder()).unwrap();
    assert!(round.widths.value(C::Folders) > 0. && round.widths.value(C::Files) > 0.);
}

#[test]
fn group441_explicit_hidden_and_zero_preferences_survive_and_can_be_restored() {
    use columns::{DetailsColumn as C, DetailsColumnPrefs as P};
    let explicit = r#"{"widths":{"name":321,"size":88,"modified":140,"type_w":110,"folders":0,"files":91,"hidden":[false,false,false,false,true]},"order":["Files","Type","Size","Folders","Modified"]}"#;
    let prefs: P = serde_json::from_str(explicit).unwrap();
    assert_eq!(prefs.widths.value(C::Folders), 0.);
    assert_eq!(prefs.widths.value(C::Files), 0.);
    let mut round = P::from_folder(&prefs.to_folder()).unwrap();
    assert_eq!(round.order, prefs.order);
    assert_eq!(round.widths.value(C::Folders), 0.);
    assert_eq!(round.widths.value(C::Files), 0.);
    round.widths.set_visible(C::Files, true);
    assert!((round.widths.files - 91.).abs() < 0.01);
    round.widths.set_visible(C::Folders, true);
    assert!(round.widths.folders > 0.);
    let folder: atlas_model::profiles::FolderColumns = serde_json::from_str(r#"{"ratios":[0.5,0.15,0.2,0.15],"order":["type","size","modified"],"hidden":["files"]}"#).unwrap();
    let prefs = P::from_folder(&folder).unwrap();
    assert!(prefs.widths.value(C::Folders) > 0.);
    assert_eq!(prefs.widths.value(C::Files), 0.);
    assert_eq!(prefs.widths.name, 395.);
}

#[test]
fn group441_popup_selection_covers_every_view_and_wheel_is_not_selection() {
    use atlas_model::settings::ViewMode;
    use selector_policy::{keyboard_mode, MODES};
    let mut mode = ViewMode::Details;
    for next in &MODES[1..] {
        mode = keyboard_mode(&mode, "down").unwrap();
        assert_eq!(&mode, next);
    }
    assert_eq!(keyboard_mode(&mode, "down"), Some(ViewMode::Tree));
    assert_eq!(keyboard_mode(&mode, "home"), Some(ViewMode::Details));
    assert_eq!(keyboard_mode(&ViewMode::Details, "up"), Some(ViewMode::Details));
    assert_eq!(keyboard_mode(&ViewMode::Details, "end"), Some(ViewMode::Tree));
    assert_eq!(keyboard_mode(&ViewMode::Tree, "up"), Some(ViewMode::Miller));
    for key in ["wheel", "scroll", "enter", "escape", "left", "right"] {
        assert_eq!(keyboard_mode(&mode, key), None);
    }
    for value in [-100., 0., 1., 2., 3.] {
        assert_eq!(selector_policy::grid_thumbnail_size(value), 64.);
    }
    assert_eq!(selector_policy::grid_thumbnail_size(4.), 80.);
    assert_eq!(selector_policy::grid_thumbnail_size(23.), 384.);
    assert_eq!(selector_policy::grid_thumbnail_size(100.), 384.);
}

#[test]
fn columns_default_legacy_and_persistence_preserve_explicit_preferences() {
    use columns::{DetailsColumn as C, DetailsColumnPrefs as P};
    let defaults = P::default();
    assert!(defaults.widths.value(C::Folders) > 0.0 && defaults.widths.value(C::Files) > 0.0);
    let old = r#"{"widths":{"name":321,"size":88,"modified":140,"type_w":110},"order":["Type","Size","Modified"]}"#;
    let legacy: P = serde_json::from_str(old).unwrap();
    assert_eq!(
        legacy.order,
        [C::Type, C::Size, C::Modified, C::Folders, C::Files]
    );
    assert_eq!(legacy.widths.name, 321.0);
    assert!(legacy.widths.folders > 0.0);
    assert!(legacy.widths.files > 0.0);
    let mut customized = defaults;
    customized.order = [C::Files, C::Size, C::Folders, C::Modified, C::Type];
    customized.widths.resize("folders", 100.0, 790.0);
    let edited_width = customized.widths.folders;
    let hidden_width = customized.widths.files;
    customized.widths.set_visible(C::Files, false);
    let restored: P = serde_json::from_str(&serde_json::to_string(&customized).unwrap()).unwrap();
    assert_eq!(restored.order, customized.order);
    assert_eq!(restored.widths.folders, edited_width);
    assert_eq!(restored.widths.value(C::Files), 0.0);
    let folder: atlas_model::profiles::FolderColumns =
        serde_json::from_str(&serde_json::to_string(&restored.to_folder()).unwrap()).unwrap();
    let round = P::from_folder(&folder).unwrap();
    assert_eq!(round.order, restored.order);
    assert_eq!(round.widths.value(C::Files), 0.0);
    assert!((round.widths.projected(790.0).folders - edited_width).abs() < 0.001);
    let mut shown = round.widths;
    shown.set_visible(C::Files, true);
    assert!((shown.files - hidden_width).abs() < 0.001);
    let legacy_folder: atlas_model::profiles::FolderColumns = serde_json::from_str(
        r#"{"ratios":[0.5,0.15,0.2,0.15],"order":["type","size","modified"]}"#,
    )
    .unwrap();
    let migrated = P::from_folder(&legacy_folder).unwrap();
    assert_eq!(migrated.order, legacy.order);
    assert!(migrated.widths.files > 0.0);
    let mut enabled = migrated.widths;
    enabled.set_visible(C::Folders, true);
    assert!(enabled.folders > 0.0);
    // Sort preferences use the same stable serde contract as tab/folder sessions.
    for sort in [SortColumn::Folders, SortColumn::Files] {
        let pref = atlas_model::profiles::FolderViewPref {
            view_mode: atlas_model::settings::ViewMode::Details,
            sort_col: sort.clone(),
            sort_dir: SortDir::Desc,
            recursive: false,
            columns: Some(restored.to_folder()),
        };
        let again: atlas_model::profiles::FolderViewPref =
            serde_json::from_str(&serde_json::to_string(&pref).unwrap()).unwrap();
        assert_eq!(again.sort_col, sort);
        assert_eq!(again.sort_dir, SortDir::Desc);
    }
}

#[test]
fn subtree_counts_reuse_and_sort_from_the_same_manual_scan() {
    use atlas_model::folder_stats::Metric;
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("A");
    std::fs::create_dir_all(root.join("large/deep")).unwrap();
    std::fs::create_dir_all(root.join("small")).unwrap();
    std::fs::write(root.join("large/a"), [0; 3]).unwrap();
    std::fs::write(root.join("large/deep/b"), [0; 5]).unwrap();
    std::fs::write(root.join("small/c"), [0; 7]).unwrap();
    let mut cache = Cache::default();
    let now = Instant::now();
    let ticket = cache.begin(&[root.clone()], now).unwrap();
    let inbox = Mutex::new(Inbox::new(&[root.clone()]));
    atlas_core::fs::folder_size::scan_with_stats(
        vec![root.clone()],
        &AtomicBool::new(false),
        |_| false,
        |_| {},
        |s| inbox.lock().unwrap().push(s),
    );
    for s in inbox.lock().unwrap().drain() {
        assert!(cache.publish(ticket, s, Instant::now()));
    }
    cache.finish_missing(ticket, false, Instant::now());
    let a = root.join("large");
    let b = root.join("small");
    assert_eq!(
        cache.metric_cell(&root, Metric::Folders, now),
        SizeCell::Exact(3)
    );
    assert_eq!(
        cache.metric_cell(&root, Metric::Files, now),
        SizeCell::Exact(3)
    );
    assert_eq!(
        cache.metric_cell(&a, Metric::Folders, now),
        SizeCell::Exact(1)
    );
    assert_eq!(
        presentation::count(cache.metric_cell(&a, Metric::Files, now)),
        "2"
    );
    assert_eq!(presentation::text(cache.cell(&a, now)), "8 B");
    let mut rows = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(atlas_core::fs::list::list_dir(
            &root,
            true,
            SortColumn::Name,
            SortDir::Asc,
            FolderOrder::Mixed,
        ))
        .unwrap();
    for (metric, column) in [
        (Metric::Folders, SortColumn::Folders),
        (Metric::Files, SortColumn::Files),
    ] {
        for direction in [SortDir::Asc, SortDir::Desc] {
            atlas_core::fs::list::sort_entries_with_sizes(
                &mut rows,
                &column,
                &direction,
                FolderOrder::Mixed,
                |e| match cache.metric_cell(&e.path(), metric, Instant::now()) {
                    SizeCell::Exact(n) | SizeCell::Cached(n) => Some(n),
                    _ => None,
                },
            );
            assert_eq!(
                rows[0].path(),
                if direction == SortDir::Asc {
                    b.clone()
                } else {
                    a.clone()
                }
            );
        }
    }
    let later = now + MAX_AGE + std::time::Duration::from_secs(86400);
    assert_eq!(
        cache.metric_cell(&a, Metric::Files, later),
        SizeCell::Cached(2)
    );
    assert!(presentation::subdued(cache.metric_cell(
        &a,
        Metric::Folders,
        later
    )));
    assert_eq!(presentation::count(SizeCell::Partial(3)), "\u{2265} 3");
    assert_eq!(presentation::count(SizeCell::Calculating), "\u{2026}");
    cache.invalidate(Some(&a.join("deep/b")));
    assert_eq!(
        presentation::count(cache.metric_cell(&a, Metric::Files, later)),
        "\u{2014}"
    );
    assert_eq!(
        cache.metric_cell(&b, Metric::Files, later),
        SizeCell::Cached(1)
    );
    // Entering A only reads retained results; this later explicit request is still allowed.
    assert!(cache.begin(&[b], later).is_some());
}

#[test]
fn selection_totals_include_folders_without_overlap_and_mark_incomplete_snapshots() {
    use atlas_model::{
        folder_selection::total, folder_size::SizeReport, folder_stats::DirectoryStats,
    };
    let now = Instant::now();
    let root = PathBuf::from("fixture/root");
    let directory = root.join("A");
    let child = directory.join("child");
    let other = root.join("other");
    let folder = entry(directory.clone(), true, 0);
    let file = entry(other, false, 7);
    let selected_child_file = entry(child.join("included"), false, 3);
    let selected_child = entry(child.clone(), true, 0);
    let mut cache = Cache::default();
    let mut stats = DirectoryStats::new(
        directory.clone(),
        SizeReport {
            bytes: 10,
            files: 2,
            folders: 1,
            done: true,
            ..Default::default()
        },
    );
    stats.observed_at = now;
    let ticket = cache.begin(&[directory.clone()], now).unwrap();
    cache.publish(ticket, stats.clone(), now);
    cache.finish_missing(ticket, false, now);
    let sum = total(
        &[
            file.clone(),
            folder.clone(),
            selected_child_file,
            selected_child,
            folder.clone(),
        ],
        &cache,
        now,
    );
    assert_eq!((sum.bytes, sum.complete, sum.cached), (17, true, false));
    let later = now + MAX_AGE;
    let aged = total(&[folder.clone(), file.clone()], &cache, later);
    assert_eq!((aged.bytes, aged.complete, aged.cached), (17, true, true));
    cache.invalidate(Some(&directory));
    let unknown = total(&[folder.clone(), file.clone()], &cache, later);
    assert_eq!((unknown.bytes, unknown.complete), (7, false));
    assert!(!total(&[folder.clone()], &cache, later).show());
    let ticket = cache.begin(&[directory.clone()], later).unwrap();
    stats.report.skipped = 1;
    stats.state = atlas_model::folder_stats::State::Partial;
    stats.observed_at = later;
    cache.publish(ticket, stats, later);
    cache.finish_missing(ticket, false, later);
    let partial = total(&[folder, file], &cache, later);
    assert_eq!((partial.bytes, partial.complete), (17, false));
    assert_eq!(
        total(&[entry(root.join("zero"), false, 0)], &cache, later).bytes,
        0
    );
}

#[path = "../../atlas-ui/src/ui/dg/table.rs"]
pub mod column_fit;
// Supply the same pure projection dependency used by the actual column policy.
mod ui {
    pub mod dg {
        pub use crate::column_fit as table;
    }
}

#[test]
fn visible_columns_resize_and_fit_without_mutating_saved_preferences() {
    use columns::{ColWidths, DetailsColumn as C, DetailsColumnPrefs as P};
    let mut prefs = P::default();
    prefs.widths.set_visible(C::Folders, false);
    prefs.widths.set_visible(C::Modified, false);
    prefs.order = [C::Files, C::Size, C::Type, C::Modified, C::Folders];
    prefs.widths.resize("files", 96.0, 790.0);
    let saved = serde_json::to_string(&prefs).unwrap();
    for width in [0.0, 120.0, 222.0, 480.0, 790.0, 1400.0] {
        let fitted = columns::responsive_widths(width, 1.0, prefs.widths, &prefs.order);
        assert_eq!(fitted[2], 0.0); // Modified hidden
        assert_eq!(fitted[4], 0.0); // Folders hidden
        assert!(fitted.iter().all(|n| n.is_finite() && *n >= 0.0));
        assert!(fitted.iter().sum::<f32>() <= width + 0.01);
    }
    assert_eq!(serde_json::to_string(&prefs).unwrap(), saved);
    let legacy: ColWidths =
        serde_json::from_str(r#"{"name":300,"size":90,"modified":150,"type_w":110}"#).unwrap();
    let fitted = columns::responsive_widths(650.0, 1.0, legacy, &C::DEFAULT_ORDER);
    assert!(fitted[4] > 0.0 && fitted[5] > 0.0);
    assert!((fitted.iter().sum::<f32>() - 650.0).abs() < 0.01);
}

#[cfg(windows)]
#[test]
fn automatic_root_policy_excludes_system_names_and_reparse_viewport_entries() {
    use atlas_model::automatic_stats::{protected_path, visible_folders};
    for root in [r"C:\", r"D:\", r"\\?\D:\", r"\\server\share\"] {
        let root = std::path::Path::new(root);
        let mut entries = vec![
            entry(root.join("Users"), true, 0),
            entry(root.join("Projects"), true, 0),
        ];
        for name in [
            "Windows",
            "Program Files",
            "Program Files (x86)",
            "ProgramData",
            "System Volume Information",
            "Recovery",
            "$Recycle.Bin",
            "RECYCLER",
        ] {
            entries.push(entry(root.join(name), true, 0));
            assert!(protected_path(&root.join(name).join("nested")));
        }
        assert_eq!(
            visible_folders(root, &entries, &(0..entries.len()).collect::<Vec<_>>()),
            [entries[0].path(), entries[1].path()]
        );
        assert!(!protected_path(&root.join("Windows-old")));
        assert!(!protected_path(&root.join("Projects/Windows")));
        let link: FileEntry = FileEntryData {
            name: "link".into(),
            path: root.join("link"),
            kind: FileKind::Dir,
            size: None,
            modified: None,
            _is_hidden: false,
            _is_symlink: true,
            archive_root: None,
            recycle_info: None,
        }
        .into();
        assert!(visible_folders(root, &[link], &[0]).is_empty());
    }
}

#[test]
fn automatic_parallel_batch_is_bounded_and_has_one_terminal_report() {
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;
    let fixture = tempfile::tempdir().unwrap();
    let paths: Vec<_> = ["first", "second", "third"]
        .map(|name| fixture.path().join(name))
        .into_iter()
        .collect();
    for (i, path) in paths.iter().enumerate() {
        std::fs::create_dir(path).unwrap();
        std::fs::write(path.join("file"), vec![0; i + 3]).unwrap();
    }
    let rendezvous = (Mutex::new(0), Condvar::new());
    let progress = Mutex::new(Vec::new());
    let ready = Mutex::new(Vec::new());
    let report = atlas_core::fs::folder_size::scan_automatic(
        paths.clone(),
        &AtomicBool::new(false),
        99,
        |r| progress.lock().unwrap().push(r),
        |stats| {
            // A serial implementation fails this timed rendezvous rather than hanging.
            let (lock, wake) = &rendezvous;
            let mut arrived = lock.lock().unwrap();
            *arrived += 1;
            wake.notify_all();
            let (arrived, timeout) = wake
                .wait_timeout_while(arrived, Duration::from_secs(5), |n| *n < 2)
                .unwrap();
            assert!(
                !timeout.timed_out() && *arrived == 2,
                "two bounded workers must overlap"
            );
            ready.lock().unwrap().push(stats);
        },
    );
    assert_eq!((report.bytes, report.files, report.folders), (7, 2, 0));
    assert_eq!(ready.lock().unwrap().len(), 2); // no per-folder task fanout
    let progress = progress.into_inner().unwrap();
    assert_eq!(progress.iter().filter(|r| r.done).count(), 1);
    assert!(progress.last().unwrap().done);
    assert!(!ready.lock().unwrap().iter().any(|s| s.path == paths[2]));
}

#[test]
fn automatic_batch_cancellation_retains_completed_children_and_waits_for_every_worker() {
    use atlas_model::{folder_size::SizeReport, folder_stats::DirectoryStats};
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir_all(a.join("child")).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::write(a.join("child/data"), [0; 9]).unwrap();
    let cancel = AtomicBool::new(false);
    let results = Mutex::new(Vec::<DirectoryStats>::new());
    let progress = Mutex::new(Vec::<SizeReport>::new());
    let mut cache = Cache::default();
    let ticket = cache
        .begin(&[a.clone(), b.clone()], Instant::now())
        .unwrap();
    let report = atlas_core::fs::folder_size::scan_automatic(
        vec![a.clone(), b.clone()],
        &cancel,
        2,
        |r| progress.lock().unwrap().push(r),
        |s| {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            results.lock().unwrap().push(s);
        },
    );
    assert!(report.done && report.cancelled);
    assert_eq!(
        progress.lock().unwrap().iter().filter(|r| r.done).count(),
        1
    );
    assert!(cache
        .begin(&[root.join("manual")], Instant::now())
        .is_none());
    for s in results.into_inner().unwrap() {
        cache.publish(ticket, s, Instant::now());
    }
    cache.finish_missing(ticket, true, Instant::now());
    for path in [&a, &b] {
        cache.retry_cancelled(path);
    }
    assert!(cache.begin(&[a, b], Instant::now()).is_some()); // manual can reserve only after terminal acknowledgment
}

#[cfg(windows)]
#[test]
fn automatic_wrapper_refuses_volume_root_and_junction_cycle() {
    let fixture = tempfile::tempdir().unwrap();
    let dir = fixture.path();
    std::fs::write(dir.join("file"), [0; 3]).unwrap();
    let junction = dir.join("cycle");
    let made = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&junction)
        .arg(dir)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let report = atlas_core::fs::folder_size::scan_automatic(
        vec![dir.to_path_buf()],
        &AtomicBool::new(false),
        1,
        |_| {},
        |_| {},
    );
    assert_eq!((report.bytes, report.files, report.skipped), (3, 1, 1));
    let link = atlas_core::fs::folder_size::scan_automatic(
        vec![junction.clone()],
        &AtomicBool::new(false),
        1,
        |_| {},
        |_| {},
    );
    assert_eq!((link.bytes, link.skipped), (0, 1));
    std::fs::remove_dir(&junction).unwrap();
    let volume = dir.ancestors().last().unwrap();
    let refused = atlas_core::fs::folder_size::scan_automatic(
        vec![volume.to_path_buf()],
        &AtomicBool::new(false),
        2,
        |_| {},
        |_| {},
    );
    assert_eq!((refused.bytes, refused.files, refused.skipped), (0, 0, 1));
}

#[test]
fn cancelled_open_descendants_retry_but_completed_and_manual_snapshots_survive() {
    use atlas_model::{folder_size::SizeReport, folder_stats::DirectoryStats};
    let now = Instant::now();
    let root = PathBuf::from("fixture/auto");
    let child = root.join("open/child");
    let done = root.join("completed");
    let manual = PathBuf::from("fixture/manual");
    let mut cache = Cache::default();
    let m = cache.begin(&[manual.clone()], now).unwrap();
    cache.finish_missing(m, true, now);
    let ticket = cache.begin(&[root.clone()], now).unwrap();
    cache.publish(
        ticket,
        DirectoryStats::new(
            child.clone(),
            SizeReport {
                done: true,
                cancelled: true,
                bytes: 3,
                ..Default::default()
            },
        ),
        now,
    );
    cache.publish(
        ticket,
        DirectoryStats::new(
            done.clone(),
            SizeReport {
                done: true,
                bytes: 7,
                ..Default::default()
            },
        ),
        now,
    );
    cache.finish_missing(ticket, true, now);
    cache.retry_cancelled_scan(ticket);
    assert_eq!(cache.cell(&root, now), SizeCell::Unknown);
    assert_eq!(cache.cell(&child, now), SizeCell::Unknown);
    assert_eq!(cache.cell(&done, now), SizeCell::Exact(7));
    assert_eq!(cache.cell(&manual, now), SizeCell::Cancelled);
    assert!(cache.begin(&[child], now).is_some());
}

#[test]
fn regression432_completed_manual_value_survives_unrelated_automatic_descendants() {
    use atlas_model::{folder_size::SizeReport, folder_stats::DirectoryStats};
    let fixture = tempfile::tempdir().unwrap();
    let now = Instant::now();
    let manual = fixture.path().join("manual");
    let automatic = fixture.path().join("automatic");
    let completed = |path: PathBuf| {
        DirectoryStats::new(
            path,
            SizeReport {
                done: true,
                bytes: 7,
                files: 1,
                ..Default::default()
            },
        )
    };
    let mut cache = Cache::default();
    let m = cache.begin(&[manual.clone()], now).unwrap();
    cache.publish(m, completed(manual.clone()), now);
    cache.finish_missing(m, false, now);
    cache.set_visible([manual.clone(), automatic.clone()]);
    let a = cache.begin_automatic(&[automatic.clone()], now).unwrap();
    for i in 0..700 {
        // Rendering/selection repeatedly reads the visible manual result.
        cache.cell(&manual, now);
        cache.publish(a, completed(automatic.join(format!("deep/{i}"))), now);
    }
    cache.publish(a, completed(automatic), now);
    cache.finish_missing(a, false, now);
    assert_eq!(
        cache.cell(&manual, now),
        SizeCell::Exact(7),
        "unchanged manual value was erased by unrelated automatic descendants"
    );
}

#[test]
fn regression432_automatic_child_does_not_invalidate_completed_parent() {
    use atlas_model::{folder_size::SizeReport, folder_stats::DirectoryStats};
    let fixture = tempfile::tempdir().unwrap();
    let now = Instant::now();
    let parent = fixture.path().join("parent");
    let child = parent.join("child");
    let mut cache = Cache::default();
    let m = cache.begin(&[parent.clone()], now).unwrap();
    cache.publish(
        m,
        DirectoryStats::new(
            parent.clone(),
            SizeReport {
                done: true,
                bytes: 7,
                files: 1,
                ..Default::default()
            },
        ),
        now,
    );
    cache.finish_missing(m, false, now);
    cache.set_visible([parent.clone(), child.clone()]);
    let _automatic = cache.begin_automatic(&[child], now).unwrap();
    assert_eq!(
        cache.cell(&parent, now),
        SizeCell::Exact(7),
        "merely calculating a child invalidated a completed parent without a filesystem change"
    );
}

#[test]
fn regression432_idle_volume_root_viewport_eventually_stops_requesting_batches() {
    use atlas_model::{
        automatic_stats::{visible_folders, RetryWindow},
        folder_size::SizeReport,
        folder_stats::DirectoryStats,
    };
    let fixture = tempfile::tempdir().unwrap();
    let volume = fixture.path().ancestors().last().unwrap();
    // In-memory entries only: never create test directories at a real volume root.
    let entries: Vec<_> = (0..4)
        .map(|i| entry(volume.join(format!("FileOP-loop-fixture-{i}")), true, 0))
        .collect();
    let visible = visible_folders(volume, &entries, &[0, 1, 2, 3]);
    assert_eq!(visible.len(), 4);
    let mut cache = Cache::default();
    let mut retry = RetryWindow::default();
    cache.set_visible(visible.clone());
    let mut scans = 0;
    let start = Instant::now();
    for cycle in 0..12 {
        let now = start + std::time::Duration::from_secs(cycle * 6);
        let batch: Vec<_> = visible
            .iter()
            .filter(|p| cache.automatic_candidate(p, now) && retry.ready(p, now))
            .take(2)
            .cloned()
            .collect();
        if batch.is_empty() {
            continue;
        }
        scans += batch.len();
        let ticket = cache.begin_automatic(&batch, now).unwrap();
        for path in &batch {
            for i in 0..700 {
                cache.publish(
                    ticket,
                    DirectoryStats::new(
                        path.join(format!("deep/{i}")),
                        SizeReport {
                            done: true,
                            ..Default::default()
                        },
                    ),
                    now,
                );
            }
            cache.publish(
                ticket,
                DirectoryStats::new(
                    path.clone(),
                    SizeReport {
                        done: true,
                        bytes: 7,
                        files: 1,
                        ..Default::default()
                    },
                ),
                now,
            );
        }
        cache.finish_missing(ticket, false, now);
        retry.defer(&batch, now);
    }
    assert_eq!(
        scans, 4,
        "an unchanged four-folder viewport kept launching replacement scans"
    );
    for path in visible {
        assert_eq!(
            cache.exact_bytes(&path, start + std::time::Duration::from_secs(72)),
            Some(7)
        );
    }
}

#[test]
fn regression432_two_real_ssd_workers_overflow_without_erasing_visible_manual_results() {
    use atlas_model::{
        folder_size::SizeReport,
        folder_stats::{DirectoryStats, Metric},
    };
    use std::sync::Arc;
    let fixture = tempfile::tempdir().unwrap();
    let now = Instant::now();
    let manual = fixture.path().join("manual");
    std::fs::create_dir(&manual).unwrap();
    std::fs::write(manual.join("data"), vec![0; 99]).unwrap();
    let roots = [fixture.path().join("first"), fixture.path().join("second")];
    for (index, root) in roots.iter().enumerate() {
        for i in 0..400 {
            std::fs::create_dir_all(root.join(format!("deep/{i}"))).unwrap();
        }
        std::fs::write(root.join("data"), vec![0; index + 3]).unwrap();
    }
    let mut cache = Cache::default();
    let m = cache.begin(&[manual.clone()], now).unwrap();
    atlas_core::fs::folder_size::scan_with_stats(
        vec![manual.clone()],
        &AtomicBool::new(false),
        |_| false,
        |_| {},
        |stats| {
            cache.publish(m, stats, Instant::now());
        },
    );
    cache.finish_missing(m, false, Instant::now());
    let mut visible = vec![manual.clone(), roots[0].clone(), roots[1].clone()];
    cache.set_visible(visible.clone());
    let ticket = cache.begin_automatic(&roots, Instant::now()).unwrap();
    let inbox = Arc::new(Mutex::new(Inbox::new(&roots)));
    let worker_inbox = inbox.clone();
    let worker_roots = roots.to_vec();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        atlas_core::fs::folder_size::scan_automatic(
            worker_roots,
            &AtomicBool::new(false),
            2,
            |report| {
                tx.send(report).unwrap();
            },
            |stats| worker_inbox.lock().unwrap().push(stats),
        )
    });
    let mut terminal = 0;
    for report in rx {
        for stats in inbox.lock().unwrap().drain() {
            cache.publish(ticket, stats, Instant::now());
        }
        assert_eq!(cache.exact_bytes(&manual, Instant::now()), Some(99));
        assert!(cache.len() <= 512);
        if report.done {
            terminal += 1;
            cache.finish_missing(ticket, report.cancelled, Instant::now());
        }
    }
    let report = worker.join().unwrap();
    assert_eq!(
        (terminal, report.bytes, report.files, report.folders),
        (1, 7, 2, 802)
    );
    for (i, root) in roots.iter().enumerate() {
        assert_eq!(
            cache.exact_bytes(root, Instant::now()),
            Some((i + 3) as u64)
        );
        assert_eq!(
            cache.metric_cell(root, Metric::Folders, Instant::now()),
            SizeCell::Exact(401)
        );
    }
    // A later batch must not evict roots of either earlier worker or the manual result.
    let later = fixture.path().join("later");
    visible.push(later.clone());
    cache.set_visible(visible.clone());
    let t = cache
        .begin_automatic(&[later.clone()], Instant::now())
        .unwrap();
    for i in 0..1000 {
        cache.publish(
            t,
            DirectoryStats::new(
                later.join(format!("deep/{i}")),
                SizeReport {
                    done: true,
                    ..Default::default()
                },
            ),
            Instant::now(),
        );
    }
    cache.publish(
        t,
        DirectoryStats::new(
            later,
            SizeReport {
                done: true,
                bytes: 11,
                files: 1,
                ..Default::default()
            },
        ),
        Instant::now(),
    );
    cache.finish_missing(t, false, Instant::now());
    for cycle in 1..100 {
        let at = Instant::now() + std::time::Duration::from_secs(cycle * 6);
        cache.expire(at);
        assert!(visible
            .iter()
            .all(|path| !cache.automatic_candidate(path, at)));
        assert_eq!(cache.exact_bytes(&manual, at), Some(99));
        assert!(cache.len() <= 512);
    }
}

#[test]
fn regression432_automatic_cancellation_keeps_prior_completed_snapshots_and_retries_only_incomplete(
) {
    use atlas_model::{folder_size::SizeReport, folder_stats::DirectoryStats};
    let fixture = tempfile::tempdir().unwrap();
    let now = Instant::now();
    let root = fixture.path().join("parent");
    let manual = root.join("manual");
    let open = root.join("open");
    let mut cache = Cache::default();
    let m = cache.begin(&[manual.clone()], now).unwrap();
    cache.publish(
        m,
        DirectoryStats::new(
            manual.clone(),
            SizeReport {
                done: true,
                bytes: 17,
                files: 1,
                ..Default::default()
            },
        ),
        now,
    );
    cache.finish_missing(m, false, now);
    cache.set_visible([manual.clone(), root.clone()]);
    let t = cache.begin_automatic(&[root.clone()], now).unwrap();
    assert!(!cache.publish(
        t,
        DirectoryStats::new(
            manual.clone(),
            SizeReport {
                done: true,
                cancelled: true,
                ..Default::default()
            }
        ),
        now
    ));
    cache.publish(
        t,
        DirectoryStats::new(
            open.clone(),
            SizeReport {
                done: true,
                cancelled: true,
                ..Default::default()
            },
        ),
        now,
    );
    cache.finish_missing(t, true, now);
    cache.retry_cancelled_scan(t);
    assert_eq!(cache.cell(&manual, now), SizeCell::Exact(17));
    assert_eq!(cache.cell(&open, now), SizeCell::Unknown);
    cache.set_visible([open.clone()]); // navigate away and enter the unfinished subtree
    assert!(cache.automatic_candidate(&open, now));
    let next = cache.begin_automatic(&[open.clone()], now).unwrap();
    cache.publish(
        next,
        DirectoryStats::new(
            open.clone(),
            SizeReport {
                done: true,
                bytes: 5,
                files: 1,
                ..Default::default()
            },
        ),
        now,
    );
    cache.finish_missing(next, false, now);
    assert!(!cache.publish(
        t,
        DirectoryStats::new(
            manual.clone(),
            SizeReport {
                done: true,
                ..Default::default()
            }
        ),
        now
    ));
    cache.set_visible([root.clone(), manual.clone(), open.clone()]); // return
    assert!(cache.automatic_candidate(&root, now));
    assert!(!cache.automatic_candidate(&manual, now));
    assert!(!cache.automatic_candidate(&open, now));
}

#[test]
fn regression432_real_mutations_still_invalidate_and_partial_failures_do_not_loop() {
    use atlas_model::{folder_size::SizeReport, folder_stats::DirectoryStats};
    let fixture = tempfile::tempdir().unwrap();
    let now = Instant::now();
    let paths: Vec<_> = ["complete", "partial", "failed"]
        .map(|name| fixture.path().join(name))
        .into_iter()
        .collect();
    let mut cache = Cache::default();
    cache.set_visible(paths.clone());
    for (index, path) in paths.iter().enumerate() {
        let t = cache.begin_automatic(&[path.clone()], now).unwrap();
        let mut report = SizeReport {
            done: true,
            bytes: 7,
            files: 1,
            ..Default::default()
        };
        if index == 1 {
            report.skipped = 1;
        }
        if index == 2 {
            report.errors = 1;
            report.files = 0;
            report.bytes = 0;
        }
        cache.publish(t, DirectoryStats::new(path.clone(), report), now);
        cache.finish_missing(t, false, now);
    }
    for cycle in 0..100 {
        let at = now + std::time::Duration::from_secs(cycle * 6);
        assert!(paths
            .iter()
            .all(|path| !cache.automatic_candidate(path, at)));
    }
    std::fs::create_dir(&paths[0]).unwrap();
    let file = paths[0].join("changed");
    std::fs::write(&file, b"changed!").unwrap();
    cache.invalidate(Some(&file)); // same scoped invalidation used by watcher/FileOP operations
    assert!(cache.automatic_candidate(&paths[0], now));
    assert!(!cache.automatic_candidate(&paths[1], now));
    let t = cache.begin_automatic(&[paths[0].clone()], now).unwrap();
    cache.invalidate(Some(&file)); // mutation during calculation must not be hidden
    cache.publish(
        t,
        DirectoryStats::new(
            paths[0].clone(),
            SizeReport {
                done: true,
                bytes: 8,
                files: 1,
                ..Default::default()
            },
        ),
        now,
    );
    cache.finish_missing(t, false, now);
    assert_eq!(cache.cell(&paths[0], now), SizeCell::Stale);
    let next = cache.begin_automatic(&[paths[0].clone()], now).unwrap();
    atlas_core::fs::folder_size::scan_with_stats(
        vec![paths[0].clone()],
        &AtomicBool::new(false),
        |_| false,
        |_| {},
        |stats| {
            cache.publish(next, stats, Instant::now());
        },
    );
    cache.finish_missing(next, false, Instant::now());
    assert_eq!(cache.exact_bytes(&paths[0], Instant::now()), Some(8));
    assert!(paths
        .iter()
        .all(|path| !cache.automatic_candidate(path, Instant::now())));
    cache.invalidate(None); // explicit broad Refresh/device/operation invalidation remains effective
    assert!(paths
        .iter()
        .all(|path| cache.automatic_candidate(path, Instant::now())));
}

#[test]
fn regression432_visible_budget_is_bounded_and_excess_rows_are_not_rescanned_in_a_cycle() {
    let fixture = tempfile::tempdir().unwrap();
    let now = Instant::now();
    let paths: Vec<_> = (0..544)
        .map(|i| fixture.path().join(format!("dir-{i}")))
        .collect();
    let mut cache = Cache::default();
    cache.set_visible(paths[..128].iter().chain(paths.iter()).cloned()); // duplicates do not waste slots
    assert_eq!(
        paths
            .iter()
            .filter(|p| cache.automatic_candidate(p, now))
            .count(),
        512
    );
    assert!(cache.begin_automatic(&[paths[512].clone()], now).is_none());
    assert_eq!(cache.len(), 0);
    cache.set_visible(paths.iter().rev().cloned()); // focus/viewport reprioritization
    assert!(cache.automatic_candidate(&paths[512], now));
    let t = cache.begin_automatic(&[paths[512].clone()], now).unwrap();
    cache.finish_missing(t, true, now);
    cache.retry_cancelled_scan(t);
    assert!(cache.len() <= 512);
}

#[test]
fn regression432_result_driven_size_sorting_cannot_create_an_unbounded_eviction_scan_cycle() {
    use atlas_model::{folder_size::SizeReport, folder_stats::DirectoryStats};
    let fixture = tempfile::tempdir().unwrap();
    let mut paths: Vec<_> = (0..600)
        .map(|i| fixture.path().join(format!("dir-{i:04}")))
        .collect();
    let mut cache = Cache::default();
    cache.set_visible(paths[..16].iter().cloned());
    let now = Instant::now();
    let mut scans = 0;
    let mut idle = 0;
    for cycle in 0..300 {
        let at = now + std::time::Duration::from_secs(cycle * 6);
        // Ascending Size puts unavailable rows first. Completing a row therefore
        // changes visible identities even though the user never scrolls/navigates.
        paths.sort_by_key(|path| cache.exact_bytes(path, at).is_some());
        let viewport = paths[..16].to_vec();
        cache.update_visible(viewport.clone());
        let batch: Vec<_> = viewport
            .into_iter()
            .filter(|path| cache.automatic_candidate(path, at))
            .take(2)
            .collect();
        if batch.is_empty() {
            idle += 1;
            continue;
        }
        let ticket = cache.begin_automatic(&batch, at).unwrap();
        scans += batch.len();
        for path in batch {
            cache.publish(
                ticket,
                DirectoryStats::new(
                    path,
                    SizeReport {
                        done: true,
                        bytes: 7,
                        files: 1,
                        ..Default::default()
                    },
                ),
                at,
            );
        }
        cache.finish_missing(ticket, false, at);
        assert!(cache.len() <= 512);
    }
    assert_eq!(scans, 512);
    assert!(idle >= 40);
    assert_eq!(cache.automatic_slots(), 0);
    let visible = paths[0].clone();
    cache.invalidate(Some(&visible.join("changed")));
    assert!(cache.automatic_candidate(&visible, now)); // a genuine change replenishes the budget
    cache.set_visible([visible.clone()]); // explicit navigation/scroll/focus also replenishes it
    let ticket = cache.begin_automatic(&[visible], now).unwrap();
    cache.finish_missing(ticket, true, now);
    cache.retry_cancelled_scan(ticket);
}
