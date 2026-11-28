use atlas_core::fs::stats_cache::{load, save, MAX_BYTES};
use atlas_model::folder_stats::DiskCache;
#[test]
fn corrupt_truncated_oversized_and_unknown_caches_recover_by_atomic_replacement() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("folder-statistics.json");
    let valid = DiskCache { version: 1, entries: Vec::new() };
    assert!(load(&path).is_none());
    for bytes in [b"{broken".as_slice(), br#"{"version":2,"entries":[]}"#,
        br#"{"version":1,"entries":[{}]}"#] {
        std::fs::write(&path, bytes).unwrap();
        assert!(load(&path).is_none());
        save(&path, &valid).unwrap();
        assert_eq!(load(&path).unwrap().version, 1);
    }
    std::fs::File::create(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
    assert!(load(&path).is_none());
    save(&path, &valid).unwrap();
    assert!(load(&path).is_some());
}
#[test]
fn failed_save_retains_the_previous_complete_snapshot() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("cache.json");
    let valid = DiskCache { version: 1, entries: Vec::new() };
    save(&path, &valid).unwrap();
    let before = std::fs::read(&path).unwrap();
    let oversized = DiskCache { version: 1, entries: vec![atlas_model::folder_stats::DiskEntry {
        path: vec![65535; MAX_BYTES as usize], report: Default::default(),
        calculated_at: std::time::SystemTime::now(), invalidated: false }] };
    assert!(save(&path, &oversized).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
