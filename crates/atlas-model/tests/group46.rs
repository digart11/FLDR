use atlas_model::{
    file_entry::{FileEntry, FileEntryData, FileKind},
    file_presentation::selected_extension,
    settings::{percent_step, shortcut_alpha, Settings, Workspace},
};

#[test]
fn percent_precision_bounds_and_serialized_preferences() {
    let mut settings = Settings::default();
    for percent in [2, 3, 17, 31] {
        let value = percent as f32 / 100.0;
        settings.alternating_row_strength = percent_step(value, 0., 1.);
        settings.detail_separator_strength = percent_step(value, 0., 1.);
        settings.file_selection_opacity = percent_step(value.max(0.10), 0.10, 0.70);
        let saved = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.alternating_row_strength, value);
        assert_eq!(restored.detail_separator_strength, value);
        assert_eq!(restored.file_selection_opacity, value.max(0.10));
        let mut repeatedly_snapped = value;
        for _ in 0..100 {
            repeatedly_snapped = percent_step(repeatedly_snapped, 0., 1.);
        }
        assert_eq!(repeatedly_snapped, value);
    }
    assert_eq!(percent_step(-1., 0.1, 0.7), 0.1);
    assert_eq!(percent_step(2., 0.1, 0.7), 0.7);
    assert_eq!(shortcut_alpha(-1.), 0.);
    assert_eq!(shortcut_alpha(1.), 0.60);
    assert_eq!(shortcut_alpha(f32::NAN), 0.25);
}

#[test]
fn shortcut_profiles_restore_independently_without_resetting_legacy_preferences() {
    let mut settings: Settings = serde_json::from_str(
        r#"{"alternating_row_strength":0.173,"glass_opacity":0.55,"file_selection_opacity":0.31}"#,
    )
    .unwrap();
    assert_eq!(settings.shortcut_target_opacity, 0.25);
    assert_eq!(settings.workspace().shortcut_target_opacity, 0.25);
    let mut profiles = Vec::new();
    for percent in [0, 2, 3, 17, 31, 60] {
        settings.shortcut_target_opacity = percent as f32 / 100.;
        profiles.push(serde_json::to_string(&settings.workspace()).unwrap());
    }
    for (saved, percent) in profiles.iter().zip([0, 2, 3, 17, 31, 60]) {
        settings.apply_workspace(&serde_json::from_str::<Workspace>(saved).unwrap());
        assert_eq!(settings.shortcut_target_opacity, percent as f32 / 100.);
        assert_eq!(settings.file_selection_opacity, 0.31);
        assert_eq!(settings.alternating_row_strength, 0.173);
        assert_eq!(settings.glass_opacity, 0.55);
    }
    assert_eq!(
        serde_json::from_str::<Workspace>("{}")
            .unwrap()
            .shortcut_target_opacity,
        0.25
    );
}

fn item(name: &str, kind: FileKind) -> FileEntry {
    FileEntryData {
        name: name.into(),
        path: std::path::Path::new(r"C:\fixture").join(name),
        kind,
        size: Some(123),
        modified: None,
        _is_hidden: false,
        _is_symlink: false,
        archive_root: None,
        recycle_info: None,
    }
    .into()
}
#[test]
fn extension_pill_selection_and_filesystem_scope() {
    assert_eq!(
        selected_extension(&[item("photo.CR3", FileKind::File)], true),
        Some("CR3")
    );
    assert!(selected_extension(&[], true).is_none());
    assert!(selected_extension(&[item("file.txt", FileKind::File)], false).is_none());
    assert!(selected_extension(&[item("folder.txt", FileKind::Dir)], true).is_none());
    assert!(selected_extension(&[item("link.txt", FileKind::Symlink)], true).is_none());
    assert!(selected_extension(
        &[item("a.txt", FileKind::File), item("b.txt", FileKind::File)],
        true
    )
    .is_none());
}

#[test]
fn zero_opacity_and_virtual_entries_never_generate_resolution_work() {
    use atlas_model::shortcut_targets::{key_for, Cache};
    let link = item("project.LNK", FileKind::File);
    let mut cache = Cache::default();
    for opacity in [0.0, -1.0] {
        assert!(key_for(&link, true, opacity)
            .and_then(|key| cache.request(key, std::time::Instant::now()))
            .is_none());
    }
    assert!(key_for(&link, false, 0.25).is_none());
    assert!(key_for(&item("folder.lnk", FileKind::Dir), true, 0.25).is_none());
    assert!(key_for(&item("file.txt", FileKind::File), true, 0.25).is_none());
    assert!(key_for(&link, true, 0.01).is_some());
}
