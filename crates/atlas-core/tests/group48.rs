//! View admission and asynchronous Shell icon request regressions without GPUI.
#[path = "../../atlas-ui/src/ui/icon_requests.rs"] mod icon_requests;
use atlas_model::{automatic_stats as policy, file_entry::{FileEntry,FileEntryData,FileKind}, settings::{Settings,ViewMode},folder_stats::Cache};
use std::{path::PathBuf,time::{Instant,Duration}};
fn folder(path:PathBuf)->FileEntry {
    FileEntryData {name:path.file_name().unwrap().to_string_lossy().as_ref().into(),path,kind:FileKind::Dir,size:None,modified:None,_is_hidden:false,_is_symlink:false,archive_root:None,recycle_info:None}.into()
}
#[test]
fn visible_grid_list_and_details_folders_reach_existing_bounded_admission() {
    let temp=tempfile::tempdir().unwrap();
    let entries:Vec<_>=(0..300).map(|i|folder(temp.path().join(format!("folder-{i}")))).collect();
    let cells=vec![atlas_model::grouping::HEADER_CELL_BASE,4,5,6,7];
    for mode in [ViewMode::Grid,ViewMode::List,ViewMode::Details] {
        assert!(policy::viewport_enabled(&mode));
        // Group header, two live cells, then two overscan cells.
        let rows=policy::viewport_rows(&cells,0..cells.len(),|pos|pos<=2);
        assert_eq!(rows,[4,5]);
        let paths=policy::visible_folders(temp.path(),&entries,&rows);
        let mut cache=Cache::default();let now=Instant::now();cache.set_visible(paths.clone());
        let mut settings=Settings::default();settings.automatic_folder_statistics=true;settings.folder_statistics_ssd=true;
        assert!(policy::enabled(&settings,policy::StorageKind::Ssd));
        let candidates:Vec<_>=paths.iter().filter(|path|cache.automatic_candidate(path,now)).take(policy::concurrency(policy::StorageKind::Ssd)).cloned().collect();
        assert_eq!(candidates,[entries[4].path(),entries[5].path()]);
        assert!(cache.begin_automatic(&candidates,now).is_some());
        for path in &candidates {std::fs::create_dir_all(path).unwrap();std::fs::write(path.join("fixture"),[1,2]).unwrap();}
        let report=atlas_core::fs::folder_size::scan_automatic(candidates,&std::sync::atomic::AtomicBool::new(false),2, |_|{}, |_|{});
        assert!(report.done);assert_eq!(report.bytes,4);

        assert!(policy::visible_folders(temp.path(),&entries,&[]).is_empty());
        settings.folder_statistics_network=false;
        assert!(!policy::enabled(&settings,policy::StorageKind::Network));
    }
    assert!(!policy::viewport_enabled(&ViewMode::Miller));assert!(!policy::viewport_enabled(&ViewMode::Tree));
    let auto=include_str!("../../atlas-ui/src/app/context_info/automatic.rs");
    assert!(!auto.contains("ViewMode::Details"));
    for source in [include_str!("../../atlas-ui/src/ui/views/grid_dg.rs"), include_str!("../../atlas-ui/src/ui/views/list_dg.rs")] {
        assert!(source.contains("automatic_stats::viewport_rows"));
        assert!(source.contains("observe_automatic_statistics(pane_id,"));
        assert!(!source.contains("scan_automatic("));
    }
}
#[test]
fn async_icon_identity_survives_eviction_navigation_and_reused_keys() {
    let mut requests=icon_requests::Requests::default();let now=Instant::now();
    let a=icon_requests::folder_key(std::path::Path::new(r"C:\Photos\A"));
    let b=icon_requests::folder_key(std::path::Path::new(r"C:\Photos\B"));
    assert_ne!(a,b);
    let old=requests.begin(&a,now).unwrap();let other=requests.begin(&b,now).unwrap();
    assert!(requests.begin(&a,now).is_none());
    requests.forget(&a);let current=requests.begin(&a,now).unwrap();assert_ne!(old,current);
    assert!(!requests.complete(&a,old,true,now));
    assert!(!requests.complete(&a,other,true,now));
    let mut textures=std::collections::HashMap::new();
    if requests.complete(&a,current,true,now) {textures.insert(a.clone(),"current custom icon");}
    if requests.complete(&a,old,true,now) {textures.insert(a.clone(),"stale fallback");}
    assert_eq!(textures.get(&a),Some(&"current custom icon"));
    assert!(requests.complete(&b,other,true,now));
}
#[test]
fn failed_icon_can_replace_fallback_after_bounded_cooldown() {
    let mut requests=icon_requests::Requests::default();let now=Instant::now();let key=PathBuf::from("folder");
    let first=requests.begin(&key,now).unwrap();assert!(requests.complete(&key,first,false,now));
    assert!(requests.begin(&key,now+Duration::from_secs(29)).is_none());
    let retry=requests.begin(&key,now+Duration::from_secs(30)).unwrap();
    assert!(!requests.complete(&key,first,true,now));
    assert!(requests.complete(&key,retry,true,now+Duration::from_secs(30)));
}
