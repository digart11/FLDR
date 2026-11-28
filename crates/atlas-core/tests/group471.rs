//! Focused process-lifetime and presentation regressions; no UI renderer linking.
#[path = "../../atlas-ui/src/ui/context_metadata.rs"] mod context_metadata;
#[path = "../../atlas-ui/src/ui/folder_stats_presentation.rs"] mod folder_stats_presentation;
#[path = "../../atlas-ui/src/ui/grid_presentation.rs"] mod grid_presentation;
use atlas_model::folder_stats::{Cache, DiskCache, DiskEntry, SizeCell};
use atlas_core::fs::stats_cache;
#[test]
fn persistence_child() {
    let Some(path) = std::env::var_os("FILEOP_GROUP471_FIXTURE") else { return; };
    let path = std::path::PathBuf::from(path);
    let folder = path.parent().unwrap().join("Folder");
    let disk = DiskCache { version: 1, entries: vec![DiskEntry {
        path: folder.to_str().unwrap().encode_utf16().collect(),
        report: atlas_model::folder_size::SizeReport { bytes: 12345, files: 7, folders: 3, done: true, ..Default::default() },
        calculated_at: std::time::SystemTime::now(), invalidated: false,
    }] };
    let (tx, rx) = flume::bounded(1);
    // A full queue at shutdown used to drop the final snapshot.
    tx.send(stats_cache::WriteRequest::snapshot(DiskCache { version: 1, entries: vec![] })).unwrap();
    let dest = path.clone();
    std::thread::spawn(move || stats_cache::writer_loop(Some(dest), rx));
    stats_cache::flush(&tx, disk).unwrap();
    // Deliberately skip thread/runtime destructors, like the application's OS exit path.
    std::process::exit(0);
}
#[test]
fn completed_statistics_survive_real_exit_and_fresh_model_initialization() {
    let fixture = tempfile::tempdir().unwrap();let path = fixture.path().join("cache.json");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "persistence_child", "--nocapture"])
        .env("FILEOP_GROUP471_FIXTURE", &path).status().unwrap();
    assert!(status.success());
    let mut cache = Cache::default();let now=std::time::Instant::now();
    cache.restore_disk(stats_cache::load(&path).unwrap(),now,std::time::SystemTime::now());
    assert_eq!(cache.cell(&fixture.path().join("Folder"),now),SizeCell::Cached(12345));
    let len=cache.len();let revision=cache.revision;
    for _ in 0..100 { assert!(grid_presentation::folder_label(&atlas_model::file_entry::FileKind::Dir,false,80.,1.,cache.cell(&fixture.path().join("Folder"),now)).is_some()); }
    assert_eq!(cache.len(),len);assert_eq!(cache.revision,revision);assert!(!cache.automatic_candidate(&fixture.path().join("Folder"),now));
}
#[test]
fn information_levels_group_available_fields_and_omit_unsupported_values() {
    use atlas_model::settings::InformationLevel::*;
    let props = [("System.Photo.CameraModel","Camera"),("System.Photo.ISOSpeed","400"),("System.Photo.ExposureTime","1/250 sec."),("System.Audio.SampleRate","48 kHz"),("System.Audio.ChannelCount","2")]
        .map(|(k,v)|(k.to_owned(),k.to_owned(),v.to_owned()));
    assert!(context_metadata::fields_at_level("cr2",&props,Off).is_empty());
    assert_eq!(context_metadata::fields_at_level("jpg",&props,Standard),["Camera"]);
    assert_eq!(context_metadata::fields_at_level("cr2",&props,Extended),["Camera","ISO 400","1/250 sec."]);
    assert_eq!(context_metadata::fields_at_level("mkv",&props,Extended),["48 kHz","Stereo"]);
    assert!(context_metadata::fields_at_level("psd",&[],Extended).is_empty());
    let mut settings=atlas_model::settings::Settings::default();settings.photo_information=Off;
    assert_eq!(context_metadata::level_for(&settings,"CR2"),Off);
    assert_eq!(context_metadata::level_for(&settings,"MKV"),Standard);
}

#[test] fn metadata_identity_rejects_deselection_switch_and_same_path_edits() {
    let path=std::path::PathBuf::from("selected.mp4");let time=std::time::SystemTime::now();let key=(path.clone(),Some(10),Some(time));
    assert!(context_metadata::selection_key_matches(Some(&key),&path,Some(10),Some(time)));
    assert!(!context_metadata::selection_key_matches(None,&path,Some(10),Some(time)));
    assert!(!context_metadata::selection_key_matches(Some(&key),std::path::Path::new("other.mp4"),Some(10),Some(time)));
    assert!(!context_metadata::selection_key_matches(Some(&key),&path,Some(11),Some(time)));
    assert!(!context_metadata::selection_key_matches(Some(&key),&path,Some(10),Some(time+std::time::Duration::from_secs(1))));
}
