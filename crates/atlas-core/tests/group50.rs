#![cfg(windows)]
use atlas_core::fs::{transfer::{self,TransferPair},transfer_safety as safety};
use atlas_model::{ops::{TransferKind,TransferOrigin,Resolution,OpErrorKind},app_state::BgResult};
use std::{path::Path,sync::{Arc,atomic::AtomicBool}};
async fn run(src:&Path,dst:&Path,kind:TransferKind,policy:Resolution)->atlas_model::ops::TransferReport {
    let (tx,rx)=flume::unbounded();
    transfer::execute(vec![TransferPair{src:src.into(),dst:dst.into(),on_conflict:policy}],kind,TransferOrigin::Paste,1,tx,Arc::new(AtomicBool::new(false)),vec![]).await;
    rx.try_iter().find_map(|event|if let BgResult::TransferComplete{report,..}=event{Some(report)}else{None}).unwrap()
}
fn junction(link:&Path,target:&Path) {
    let output=std::process::Command::new("cmd.exe").args(["/d","/c","mklink","/J"]).arg(link).arg(target).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
}
#[tokio::test]
async fn nested_junction_and_parallel_threshold_fail_closed_without_touching_either_tree() {
    let fixture=tempfile::tempdir().unwrap();let src=fixture.path().join("source");let external=fixture.path().join("target");let dst=fixture.path().join("destination");
    std::fs::create_dir(&src).unwrap();std::fs::create_dir(&external).unwrap();
    for i in 0..64 {std::fs::write(src.join(format!("{i}")),b"source").unwrap();}
    std::fs::write(external.join("sentinel"),b"unrelated").unwrap();let link=src.join("junction");junction(&link,&external);
    for kind in [TransferKind::Copy,TransferKind::Move] {
        let report=run(&src,&dst,kind,Resolution::Rename).await;
        assert!(report.completed.is_empty());assert!(report.errors.iter().any(|error|matches!(error.kind,OpErrorKind::UnsupportedEntry(_))));
        assert!(!dst.exists());assert_eq!(std::fs::read_dir(&src).unwrap().count(),65);
        assert_eq!(std::fs::read(external.join("sentinel")).unwrap(),b"unrelated");
    }
    std::fs::remove_dir(&link).unwrap();
}
#[tokio::test]
#[ignore = "requires Windows symlink privilege or existing Developer Mode; explicit prerequisite failed with ERROR_PRIVILEGE_NOT_HELD here"]
async fn directory_file_and_broken_symlinks_are_detected_in_planning_and_execution() {
    let fixture=tempfile::tempdir().unwrap();let target=fixture.path().join("file");let dir=fixture.path().join("dir");let output=fixture.path().join("out");
    std::fs::write(&target,b"source").unwrap();std::fs::create_dir(&dir).unwrap();
    let file_link=fixture.path().join("file-link");let dir_link=fixture.path().join("dir-link");let broken=fixture.path().join("broken");
    // Windows Developer Mode or symlink privilege is required; report an ignored
    // prerequisite through a separate test when unavailable, never fake coverage.
    std::os::windows::fs::symlink_file(&target,&file_link).expect("Windows symlink privilege/Developer Mode required for this fixture");
    std::os::windows::fs::symlink_dir(&dir,&dir_link).unwrap();
    std::os::windows::fs::symlink_file(fixture.path().join("absent"),&broken).unwrap();
    for path in [&file_link,&dir_link,&broken] {
        let request=atlas_model::ops::TransferRequest{sources:vec![path.clone()],target_names:None,dest:fixture.path().into(),kind:TransferKind::Copy,policy:atlas_model::ops::ConflictPolicy::RenameAll,origin:TransferOrigin::Paste};
        assert!(transfer::validate(&request).errors.iter().any(|error|matches!(error.kind,OpErrorKind::UnsupportedEntry(_))));
        let report=run(path,&output,TransferKind::Move,Resolution::Rename).await;
        assert!(report.completed.is_empty());assert!(report.errors.iter().any(|error|matches!(error.kind,OpErrorKind::UnsupportedEntry(_))));
        assert!(std::fs::symlink_metadata(path).is_ok());
    }
    std::fs::remove_file(file_link).unwrap();std::fs::remove_file(broken).unwrap();std::fs::remove_dir(dir_link).unwrap();
}
#[test]
fn destination_race_and_foreign_output_are_never_replaced_or_cleaned_up() {
    let fixture=tempfile::tempdir().unwrap();let src=fixture.path().join("source");let dst=fixture.path().join("destination");
    std::fs::write(&src,b"new").unwrap();assert!(!dst.exists());
    std::fs::write(&dst,b"appeared after planning").unwrap();
    let error=safety::copy_file(&src,&dst,&AtomicBool::new(false),|_|{}).err().unwrap();
    assert_eq!(error.kind(),std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&dst).unwrap(),b"appeared after planning");
    assert!(safety::rename_new(&src,&dst).is_err());
    assert_eq!(std::fs::read(&src).unwrap(),b"new");assert_eq!(std::fs::read(&dst).unwrap(),b"appeared after planning");
}
#[test]
fn owned_output_is_locked_during_copy_and_replacement_cannot_acquire_a_certificate() {
    let fixture=tempfile::tempdir().unwrap();let src=fixture.path().join("source");let dst=fixture.path().join("destination");
    std::fs::write(&src,vec![7;2*1024*1024]).unwrap();
    let created=safety::copy_file(&src,&dst,&AtomicBool::new(false),|_| {
        assert!(std::fs::remove_file(&dst).is_err());assert!(std::fs::write(&dst,b"intruder").is_err());
        assert!(std::fs::write(&src,b"changed").is_err());
    }).unwrap();
    std::fs::remove_file(&dst).unwrap();std::fs::write(&dst,vec![7;2*1024*1024]).unwrap();
    assert!(safety::copy_certificate(&src,&dst,&created).is_err());
    assert_eq!(std::fs::read(&src).unwrap().len(),2*1024*1024);
}
#[tokio::test]
async fn replace_merge_and_uncertified_inverse_preserve_unrelated_children() {
    let fixture=tempfile::tempdir().unwrap();let src=fixture.path().join("source");let dst=fixture.path().join("destination");
    std::fs::create_dir(&src).unwrap();std::fs::create_dir(&dst).unwrap();
    std::fs::write(src.join("new"),b"new").unwrap();std::fs::write(dst.join("unrelated"),b"old").unwrap();
    for kind in [TransferKind::Copy,TransferKind::Move] {
        let report=run(&src,&dst,kind,Resolution::Overwrite).await;
        assert!(report.completed.is_empty());assert!(report.reversible.is_empty());
        assert!(report.errors.iter().any(|error|error.kind==OpErrorKind::UnsafeReplacement));
        assert_eq!(std::fs::read(dst.join("unrelated")).unwrap(),b"old");
        // Even a forged file certificate cannot authorize a directory inverse.
        let forged=atlas_model::undo::TransferUndo{source:src.clone(),destination:dst.clone(),stamp:atlas_model::undo::ObjectStamp{identity:(0,0),size:0,modified:std::time::UNIX_EPOCH,digest:[0;32]}};
        assert!(safety::undo(&forged,kind).is_err());assert_eq!(std::fs::read(dst.join("unrelated")).unwrap(),b"old");
    }
}
#[tokio::test]
async fn verified_file_copy_and_move_undo_succeed_but_changed_or_replaced_objects_are_rejected() {
    let fixture=tempfile::tempdir().unwrap();let src=fixture.path().join("source");let dst=fixture.path().join("destination");std::fs::write(&src,b"original").unwrap();
    let report=run(&src,&dst,TransferKind::Copy,Resolution::Rename).await;
    assert!(report.errors.is_empty(),"{:?}",report.errors);assert_eq!(report.reversible.len(),1);
    safety::undo(&report.reversible[0],TransferKind::Copy).unwrap();assert!(!dst.exists());assert!(src.exists());
    let report=run(&src,&dst,TransferKind::Move,Resolution::Rename).await;
    assert!(report.errors.is_empty(),"{:?}",report.errors);assert_eq!(report.reversible.len(),1);
    safety::undo(&report.reversible[0],TransferKind::Move).unwrap();assert!(src.exists());assert!(!dst.exists());
    let report=run(&src,&dst,TransferKind::Copy,Resolution::Rename).await;let proof=&report.reversible[0];
    std::fs::write(&dst,b"edited after copy").unwrap();assert!(safety::undo(proof,TransferKind::Copy).is_err());
    assert_eq!(std::fs::read(&dst).unwrap(),b"edited after copy");
}
#[tokio::test]
async fn copy_delete_fallback_retains_source_and_reports_distinct_outcome() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture=tempfile::tempdir().unwrap();let src=fixture.path().join("source");let dst=fixture.path().join("destination");std::fs::write(&src,b"original").unwrap();
    // Deny DELETE while permitting READ: force copy fallback without a second volume.
    let _source_lock=std::fs::OpenOptions::new().read(true).share_mode(1).open(&src).unwrap();
    let report=run(&src,&dst,TransferKind::Move,Resolution::Rename).await;
    assert!(report.completed.is_empty());assert_eq!(report.sources_retained,vec![(src.clone(),dst.clone())]);
    assert!(report.errors.iter().any(|error|matches!(error.kind,OpErrorKind::SourceNotRemoved(_))));
    assert_eq!(std::fs::read(&src).unwrap(),b"original");assert_eq!(std::fs::read(&dst).unwrap(),b"original");
}

#[test]
fn changed_source_version_and_failed_completion_never_authorize_removal() {
    let fixture=tempfile::tempdir().unwrap();let src=fixture.path().join("source");let dst=fixture.path().join("partial");
    std::fs::write(&src,vec![1;2*1024*1024]).unwrap();let before=safety::tree(&src).unwrap();
    let cancel=AtomicBool::new(false);
    let result=safety::copy_file(&src,&dst,&cancel,|_| {cancel.store(true,std::sync::atomic::Ordering::Relaxed);});
    assert!(result.is_err());assert!(src.exists());assert!(dst.exists());
    std::fs::write(&src,b"new version").unwrap();
    assert_eq!(safety::retained_reason(&src,&before),OpErrorKind::SourceChanged);
    assert_eq!(std::fs::read(&src).unwrap(),b"new version");
}
#[tokio::test]
async fn broken_junction_and_reparse_ancestors_never_route_output_into_a_target() {
    let fixture=tempfile::tempdir().unwrap();let target=fixture.path().join("target");let link=fixture.path().join("link");let src=fixture.path().join("source");
    std::fs::create_dir(&target).unwrap();std::fs::write(&src,b"source").unwrap();junction(&link,&target);
    let report=run(&src,&link.join("injected"),TransferKind::Copy,Resolution::Rename).await;
    assert!(report.completed.is_empty());assert!(!target.join("injected").exists());
    std::fs::remove_dir(&target).unwrap(); // disposable target: junction is now broken
    let report=run(&link,&fixture.path().join("output"),TransferKind::Copy,Resolution::Rename).await;
    assert!(report.errors.iter().any(|error|matches!(error.kind,OpErrorKind::UnsupportedEntry(_))));assert!(report.completed.is_empty());
    std::fs::remove_dir(&link).unwrap();
}
