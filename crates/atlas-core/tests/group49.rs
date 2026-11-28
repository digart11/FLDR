//! Tab material regressions and explicit characterization of existing engine risk.
#[path = "../../atlas-ui/src/ui/address_policy.rs"] mod address_policy;
#[path = "../../atlas-ui/src/ui/tab_shape.rs"] mod tab_shape;
use atlas_core::fs::transfer::{self,TransferPair};
use atlas_model::{app_state::BgResult,ops::{TransferKind,TransferOrigin,Resolution}};
use std::{sync::{Arc,atomic::AtomicBool},path::PathBuf};
#[test]
fn active_tabs_remain_connected_while_inactive_hover_stays_recessed_in_each_split() {
    for focus in [1,2,3,4] {
        for (children,local) in [([1,2],1),([3,4],3)] {
            let active=tab_shape::active_child(&children,Some(local),focus).unwrap();
            assert_eq!(active,if children.contains(&focus) {focus} else {local});
            for tab in children {for hover in [0.,0.5,1.] {
                let amount=tab_shape::surface_hover(tab==active,hover);
                for alpha in [0.,0.035,0.4,1.] {for backing in [0.02,0.5,0.95] {
                    let base=[0.4,0.4,0.4,alpha];
                    let material=if tab==active {base} else {address_policy::darker_material(base,address_policy::inactive_reduction(amount))};
                    let active_tone=base[0]*alpha+backing*(1.-alpha);
                    let tone=material[0]*material[3]+backing*(1.-material[3]);
                    if tab==active {assert_eq!(material,base);} else {
                        assert!(tone<=active_tone*0.7001);
                        assert!(tone>=active_tone*0.6499);
                    }
                }}
            }}
        }
    }
}
async fn copy(src:PathBuf,dst:PathBuf)->atlas_model::ops::TransferReport {
    let (tx,rx)=flume::unbounded();
    transfer::execute(vec![TransferPair {src,dst,on_conflict:Resolution::Rename}],TransferKind::Copy,TransferOrigin::Paste,1,tx,Arc::new(AtomicBool::new(false)),vec![]).await;
    rx.try_iter().find_map(|event|if let BgResult::TransferComplete {report,..}=event {Some(report)}else{None}).unwrap()
}
#[cfg(windows)]
#[tokio::test]
async fn parallel_threshold_tree_with_junction_is_rejected_without_omission() {
    // A COPY-only fixture: no source deletion or user directory is involved.
    let fixture=tempfile::tempdir().unwrap();
    let src=fixture.path().join("source");let target=fixture.path().join("link-target");let dst=fixture.path().join("destination");
    std::fs::create_dir(&src).unwrap();std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("sentinel"),b"preserve").unwrap();
    for i in 0..32 {std::fs::write(src.join(format!("{i}.txt")),b"x").unwrap();}
    let junction=src.join("linked-folder");
    let made=std::process::Command::new("cmd.exe").args(["/d","/c","mklink","/J"]).arg(&junction).arg(&target).output().unwrap();
    assert!(made.status.success(),"{}",String::from_utf8_lossy(&made.stderr));
    assert!(std::fs::symlink_metadata(&junction).unwrap().file_type().is_symlink());
    let report=copy(src.clone(),dst.clone()).await;
    assert!(report.completed.is_empty());assert!(report.errors.iter().any(|error|matches!(error.kind,atlas_model::ops::OpErrorKind::UnsupportedEntry(_))));
    assert!(!dst.join("linked-folder").exists());
    assert_eq!(std::fs::read(target.join("sentinel")).unwrap(),b"preserve");
    assert_eq!(std::fs::read_dir(&src).unwrap().count(),33);
    std::fs::remove_dir(&junction).unwrap(); // unlink junction before fixture cleanup
}
#[tokio::test]
async fn same_name_clean_sources_keep_both_at_execution() {
    let fixture=tempfile::tempdir().unwrap();let a=fixture.path().join("a");let b=fixture.path().join("b");let dst=fixture.path().join("destination");
    for path in [&a,&b,&dst] {std::fs::create_dir(path).unwrap();}
    let first=a.join("same.txt");let second=b.join("same.txt");
    std::fs::write(&first,b"first").unwrap();std::fs::write(&second,b"second").unwrap();
    let (conflicts,clean)=transfer::detect_conflicts(&[first.clone(),second.clone()],None,&dst);
    let built=transfer::build_pairs(&clean,&conflicts,&atlas_model::ops::ConflictPolicy::RenameAll,&dst);
    let (tx,rx)=flume::unbounded();
    transfer::execute(built.pairs,TransferKind::Copy,TransferOrigin::Paste,2,tx,Arc::new(AtomicBool::new(false)),vec![]).await;
    let report=rx.try_iter().find_map(|event|if let BgResult::TransferComplete {report,..}=event {Some(report)}else{None}).unwrap();
    assert!(report.errors.is_empty());assert_eq!(report.completed.len(),2);
    assert_ne!(report.completed[0].1,report.completed[1].1);
    assert_eq!(std::fs::read(&report.completed[0].1).unwrap(),b"first");
    assert_eq!(std::fs::read(&report.completed[1].1).unwrap(),b"second");
}
