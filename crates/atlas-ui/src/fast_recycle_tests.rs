//! Only private, newly created Temp fixtures. Never call the live-bin executor.
use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("fldr-recycle-unit-{}-{}", std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap(); Self(root)
    }
    fn path(&self, name: &str) -> PathBuf { self.0.join(name) }
    fn metadata(&self) -> File {
        OpenOptions::new().read(true).write(true).create_new(true)
            .access_mode(0x80000000 | 0x40000000 | DELETE_ACCESS).share_mode(1)
            .open(self.path("private-metadata")).unwrap()
    }
}
impl Drop for Fixture { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }

#[test]
fn local_volume_proof_accepts_a_disposable_ntfs_directory() {
    let fixture = Fixture::new();
    let root = drive_root(&fixture.0).unwrap();
    assert!(is_fixed_ntfs(&root));
    let handle = open_directory(&fixture.0, false).unwrap();
    assert!(has_local_volume_guid(&handle));
}

#[test]
fn shell_stream_reader_is_blocked_by_writer_but_works_after_handoff() {
    use windows::Win32::{UI::Shell::SHCreateStreamOnFileEx,
        System::Com::{STGM_READ, STGM_SHARE_DENY_WRITE}};
    let f = Fixture::new(); let path = f.path("private-metadata"); let mut metadata = f.metadata();
    metadata.write_all(b"exact metadata bytes").unwrap(); metadata.sync_all().unwrap();
    let id = identity(&metadata).unwrap();
    let open_shell_reader = || unsafe { SHCreateStreamOnFileEx(&HSTRING::from(path.as_os_str()),
        (STGM_READ | STGM_SHARE_DENY_WRITE).0, 0, false, None) };
    let error = open_shell_reader().err().expect("old retained writer must reproduce the sharing conflict");
    assert_eq!(error.code().0 as u32, 0x80070020); // ERROR_SHARING_VIOLATION
    let guard = metadata_read_guard(&mut metadata, &path).unwrap();
    assert_eq!(identity(&metadata).unwrap(), id);
    let stream = open_shell_reader().expect("read-only handoff must allow the Shell stream reader");
    let mut bytes = [0u8; 20]; let mut read = 0;
    unsafe { stream.Read(bytes.as_mut_ptr().cast(), bytes.len() as u32, Some(&mut read)).ok().unwrap(); }
    assert_eq!(&bytes[..read as usize], b"exact metadata bytes");
    drop(stream); guard.cleanup(&metadata).unwrap(); drop(metadata); assert!(!path.exists());
}

#[test]
fn published_metadata_allows_readers_but_blocks_write_delete_and_rename() {
    let f = Fixture::new(); let path = f.path("private-metadata"); let mut metadata = f.metadata();
    metadata.write_all(b"record").unwrap();
    let guard = metadata_read_guard(&mut metadata, &path).unwrap();
    let mut reader = OpenOptions::new().read(true).share_mode(1).open(&path).unwrap();
    let mut bytes = Vec::new(); reader.read_to_end(&mut bytes).unwrap(); assert_eq!(bytes, b"record"); drop(reader);
    assert!(metadata.write_all(b"cannot write through anchor").is_err());
    assert!(OpenOptions::new().write(true).share_mode(1|2|4).open(&path).is_err());
    assert!(fs::remove_file(&path).is_err()); assert!(fs::rename(&path,f.path("other")).is_err());
    guard.cleanup(&metadata).unwrap(); drop(metadata); assert!(!path.exists());
}

#[test]
fn cleanup_after_handoff_uses_object_identity_even_if_path_is_replaced() {
    let f=Fixture::new(); let path=f.path("private-metadata"); let moved=f.path("renamed-owned-record");
    let mut metadata=f.metadata(); metadata.write_all(b"owned").unwrap();
    let guard=metadata_read_guard(&mut metadata,&path).unwrap();
    // Deliberately release the guard to exercise the cleanup transition race.
    drop(guard.0.borrow_mut().take()); fs::rename(&path,&moved).unwrap(); fs::write(&path,b"unrelated").unwrap();
    guard.cleanup(&metadata).unwrap(); drop(metadata);
    assert!(!moved.exists()); assert_eq!(fs::read(&path).unwrap(),b"unrelated");
}

#[test]
fn handoff_rejects_a_different_file_at_reserved_path_without_deleting_it() {
    let f=Fixture::new(); let path=f.path("private-metadata"); let unrelated=f.path("unrelated");
    let mut metadata=f.metadata(); fs::write(&unrelated,b"keep").unwrap();
    assert!(metadata_read_guard(&mut metadata,&unrelated).is_err());
    delete_metadata(&metadata).unwrap(); drop(metadata);
    assert!(!path.exists()); assert_eq!(fs::read(unrelated).unwrap(),b"keep");
}

#[test]
fn published_metadata_verification_failure_rolls_back_and_cleans_owned_record() {
    let f=Fixture::new(); let original=f.path("source"); let dest=f.path("payload"); let info=f.path("private-metadata");
    fs::create_dir(&original).unwrap(); fs::write(original.join("test"),b"unchanged").unwrap();
    let source=open_directory(&original,true).unwrap(); let source_id=identity(&source).unwrap();
    let mut metadata=f.metadata(); let record=metadata_record(&original,9,1).unwrap(); metadata.write_all(&record).unwrap();
    let guard=metadata_read_guard(&mut metadata,&info).unwrap();
    let result=transact(&mut metadata,&info,&dest,&original,&AtomicBool::new(false),Some(source_id),
        |target|rename_handle(&source,target), |file|{
            verify_pair(file,&record,&source,source_id,&original,&dest)?;
            Err(io::Error::other("injected namespace rejection"))
        }, |file|guard.cleanup(file));
    assert!(matches!(result,Outcome::Error(_))); drop(metadata); drop(source);
    assert_eq!(fs::read(original.join("test")).unwrap(),b"unchanged"); assert!(!dest.exists()); assert!(!info.exists());
}

#[test]
fn published_metadata_rollback_collision_retains_recovery_pair() {
    let f=Fixture::new(); let original=f.path("source"); let dest=f.path("payload"); let info=f.path("private-metadata");
    fs::create_dir(&original).unwrap(); fs::write(original.join("original"),b"original").unwrap();
    let source=open_directory(&original,true).unwrap(); let id=identity(&source).unwrap(); let mut metadata=f.metadata();
    let guard=metadata_read_guard(&mut metadata,&info).unwrap();
    let result=transact(&mut metadata,&info,&dest,&original,&AtomicBool::new(false),Some(id),
        |target|rename_handle(&source,target), |_|{
            fs::create_dir(&original)?; fs::write(original.join("unrelated"),b"keep")?;
            Err(io::Error::other("injected namespace rejection"))
        }, |file|guard.cleanup(file));
    assert!(matches!(result,Outcome::Error(_))); drop(guard); drop(metadata); drop(source);
    assert!(info.exists()); assert_eq!(fs::read(dest.join("original")).unwrap(),b"original");
    assert_eq!(fs::read(original.join("unrelated")).unwrap(),b"keep");
}

#[test]
fn retained_payload_handle_allows_shell_filesystem_binding_and_rollback() {
    use windows::Win32::{System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED},
        UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH}};
    struct Apartment;
    impl Drop for Apartment { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap(); }
    let _apartment = Apartment;
    let f=Fixture::new(); let original=f.path("original"); let dest=f.path("payload"); fs::create_dir(&original).unwrap();
    let source=open_directory(&original,true).unwrap(); rename_handle(&source,&dest).unwrap();
    let item: IShellItem = unsafe { SHCreateItemFromParsingName(&HSTRING::from(dest.as_os_str()),None).unwrap() };
    let name=unsafe { item.GetDisplayName(SIGDN_FILESYSPATH).unwrap() };
    let physical=unsafe { name.to_string().unwrap() }; unsafe { CoTaskMemFree(Some(name.0.cast())); }
    assert!(physical.eq_ignore_ascii_case(&dest.to_string_lossy()));
    drop(item); rename_handle(&source,&original).unwrap();
}

#[test]
fn published_metadata_cancellation_cleans_before_move_and_after_rollback() {
    for initially_cancelled in [true,false] {
        let f=Fixture::new(); let original=f.path("source"); let dest=f.path("payload"); let info=f.path("private-metadata");
        fs::create_dir(&original).unwrap(); fs::write(original.join("test"),b"keep").unwrap();
        let source=open_directory(&original,true).unwrap(); let id=identity(&source).unwrap(); let mut metadata=f.metadata();
        let guard=metadata_read_guard(&mut metadata,&info).unwrap(); let cancel=AtomicBool::new(initially_cancelled); let mut moves=0;
        let result=transact(&mut metadata,&info,&dest,&original,&cancel,Some(id),
            |target|{moves+=1;rename_handle(&source,target)}, |_|{cancel.store(true,Ordering::Relaxed);Ok(())},
            |file|guard.cleanup(file));
        assert!(matches!(result,Outcome::Cancelled)); assert_eq!(moves,if initially_cancelled {0}else{2});
        drop(metadata); drop(source); assert_eq!(fs::read(original.join("test")).unwrap(),b"keep");
        assert!(!dest.exists()); assert!(!info.exists());
    }
}

#[test]
fn published_metadata_cleanup_failure_remains_explicit_and_retryable() {
    let f=Fixture::new(); let info=f.path("private-metadata"); let mut metadata=f.metadata();
    let guard=metadata_read_guard(&mut metadata,&info).unwrap();
    let blocking_reader=OpenOptions::new().read(true).share_mode(1).open(&info).unwrap();
    let result=transact(&mut metadata,&info,&f.path("payload"),&f.path("source"),&AtomicBool::new(true),None,
        |_|panic!("pre-cancelled"), |_|panic!("pre-cancelled"),|file|guard.cleanup(file));
    let Outcome::Error(message)=result else{panic!("cleanup failure must be explicit")};
    assert!(message.contains("cleanup failed")); assert!(info.exists());
    drop(blocking_reader); guard.cleanup(&metadata).unwrap(); drop(metadata); assert!(!info.exists());
}

#[test]
fn metadata_v2_unicode_roundtrip() {
    let path = Path::new("C:\\Photos\\été-📷");
    let bytes = metadata_record(path, 241, 123456789).unwrap();
    assert_eq!(&bytes[..8], &2u64.to_le_bytes());
    assert_eq!(&bytes[8..16], &241u64.to_le_bytes());
    assert_eq!(&bytes[16..24], &123456789u64.to_le_bytes());
    let units: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize, units.len());
    let decoded: Vec<_> = bytes[28..].chunks_exact(2).map(|p| u16::from_le_bytes([p[0],p[1]])).collect();
    assert_eq!(decoded, units); assert_eq!(bytes.len(), 28 + 2 * units.len());
}
#[test]
fn metadata_preserves_unpaired_utf16() {
    use std::os::windows::ffi::OsStringExt;
    let mut units: Vec<_> = "C:\\test-".encode_utf16().collect(); units.push(0xd800);
    let path = PathBuf::from(std::ffi::OsString::from_wide(&units));
    let record = metadata_record(&path, 0, 0).unwrap();
    assert_eq!(&record[record.len()-4..], &[0, 0xd8, 0, 0]);
}
#[test]
fn metadata_rejects_embedded_nul() { assert!(metadata_record(Path::new("C:\\a\0b"),0,0).is_err()); }
#[test]
fn eligibility_rejects_ambiguous_and_protected_paths() {
    for path in ["C:relative", "C:", "C:\\", "relative", "\\\\server\\share\\folder",
        "\\\\?\\C:\\folder", "C:\\a\\..\\b", "C:\\a\\.\\b", "C:\\a:stream", "C:\\a.",
        "C:\\a ", "C:\\$recycle.bin\\item", "C:\\SYSTEM VOLUME INFORMATION\\item"] {
        assert!(drive_root(Path::new(path)).is_none(), "{path}");
    }
    assert_eq!(drive_root(Path::new("C:\\ordinary\\folder")), Some(PathBuf::from("C:\\")));
}
#[test]
fn suffix_is_six_hex_digits_and_mixes_pid() {
    let a = suffix(123456789, 1, 0); let b = suffix(123456789, 2, 0);
    assert_ne!(a,b); assert_eq!(a.len(),6); assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
    let names: std::collections::HashSet<_> = (0..128).map(|n| suffix(1,2,n)).collect();
    assert_eq!(names.len(),128); // Reservation is still required for arbitrary collisions.
}
#[test]
fn nested_size_and_cancellation() {
    let f = Fixture::new(); fs::create_dir_all(f.path("nested/empty")).unwrap();
    fs::write(f.path("one"), b"abc").unwrap(); fs::write(f.path("nested/two"), b"12345").unwrap();
    assert_eq!(folder_size(&f.0, &AtomicBool::new(false)).unwrap(), 8);
    assert!(folder_size(&f.0, &AtomicBool::new(true)).is_err());
}
#[test]
fn ancestor_pin_blocks_directory_replacement() {
    let f = Fixture::new(); fs::create_dir(f.path("original")).unwrap();
    let pin = open_directory(&f.path("original"), false).unwrap();
    assert!(fs::rename(f.path("original"),f.path("replacement")).is_err()); drop(pin);
    fs::rename(f.path("original"), f.path("replacement")).unwrap();
}
#[test]
fn retained_handle_move_and_specific_verification() {
    let f=Fixture::new(); let original=f.path("original"); let dest=f.path("private-payload");
    fs::create_dir(&original).unwrap(); fs::write(original.join("test"),b"protected contents").unwrap();
    let source=open_directory(&original,true).unwrap(); let id=identity(&source).unwrap();
    let expected=metadata_record(&original,18,1).unwrap(); let mut meta=f.metadata(); meta.write_all(&expected).unwrap();
    rename_handle(&source,&dest).unwrap();
    verify_pair(&mut meta,&expected,&source,id,&original,&dest).unwrap();
    assert_eq!(fs::read(dest.join("test")).unwrap(),b"protected contents");
    rename_handle(&source,&original).unwrap();
}
#[test]
fn handle_rename_never_replaces_existing_directory() {
    let f=Fixture::new(); fs::create_dir(f.path("source")).unwrap(); fs::create_dir(f.path("existing")).unwrap();
    fs::write(f.path("existing/unrelated"),b"keep").unwrap(); let source=open_directory(&f.path("source"),true).unwrap();
    assert!(rename_handle(&source,&f.path("existing")).is_err());
    assert_eq!(fs::read(f.path("existing/unrelated")).unwrap(),b"keep");
    assert!(f.path("source").exists());
}
#[test]
fn corrupt_metadata_is_not_success() {
    let f=Fixture::new(); let original=f.path("source"); let dest=f.path("dest"); fs::create_dir(&original).unwrap();
    let source=open_directory(&original,true).unwrap(); let id=identity(&source).unwrap();
    let mut meta=f.metadata(); meta.write_all(b"bad").unwrap(); rename_handle(&source,&dest).unwrap();
    assert!(verify_pair(&mut meta,b"expected",&source,id,&original,&dest).is_err());
    rename_handle(&source,&original).unwrap();
}
#[test]
fn destination_identity_mismatch_is_rejected() {
    let f=Fixture::new(); let original=f.path("source"); let dest=f.path("other");
    fs::create_dir(&original).unwrap(); fs::create_dir(&dest).unwrap();
    let source=open_directory(&original,true).unwrap(); let id=identity(&source).unwrap();
    let mut meta=f.metadata(); meta.write_all(b"record").unwrap();
    assert!(verify_pair(&mut meta,b"record",&source,id,&original,&dest).is_err());
}
#[test]
fn exclusive_metadata_reservation_blocks_collision_and_cleanup_is_by_handle() {
    let f=Fixture::new(); let meta=f.metadata();
    assert!(OpenOptions::new().write(true).create_new(true).open(f.path("private-metadata")).is_err());
    assert!(fs::remove_file(f.path("private-metadata")).is_err());
    delete_metadata(&meta).unwrap(); drop(meta); assert!(!f.path("private-metadata").exists());
}

// Injected failures exercise the same transaction used in production. Fake rename
// closures never touch the filesystem; assertions verify the exact transition sequence.
#[test]
fn pre_cancel_never_moves_and_cleans_metadata() {
    let f=Fixture::new(); let mut meta=f.metadata();
    let result=transact(&mut meta,&f.path("private-metadata"),&f.path("dest"),&f.path("source"),&AtomicBool::new(true),None,
        |_| panic!("move on cancellation"), |_| panic!("verify before move"),delete_metadata);
    assert!(matches!(result,Outcome::Cancelled)); drop(meta); assert!(!f.path("private-metadata").exists());
}
#[test]
fn move_error_retains_recovery_metadata_without_fallback() {
    let f=Fixture::new(); let mut meta=f.metadata();
    let result=transact(&mut meta,&f.path("private-metadata"),&f.path("dest"),&f.path("source"),&AtomicBool::new(false),None,
        |_| Err(io::Error::other("injected move")), |_| panic!("verify failed move"), |_| panic!("erase ambiguous metadata"));
    assert!(matches!(result,Outcome::Error(_))); assert!(f.path("private-metadata").exists());
}
#[test]
fn failed_verification_rolls_back_then_cleans() {
    let f=Fixture::new(); let mut meta=f.metadata(); let mut targets=Vec::new();
    let result=transact(&mut meta,&f.path("private-metadata"),&f.path("dest"),&f.path("source"),&AtomicBool::new(false),None,
        |path|{targets.push(path.to_path_buf());Ok(())}, |_|Err(io::Error::other("not recognized")),delete_metadata);
    assert!(matches!(result,Outcome::Error(_))); assert_eq!(targets,vec![f.path("dest"),f.path("source")]);
    drop(meta); assert!(!f.path("private-metadata").exists());
}
#[test]
fn rollback_failure_retains_both_recovery_paths() {
    let f=Fixture::new(); let mut meta=f.metadata(); let mut calls=0;
    let result=transact(&mut meta,&f.path("private-metadata"),&f.path("dest"),&f.path("source"),&AtomicBool::new(false),None,
        |_|{calls+=1;if calls==1{Ok(())}else{Err(io::Error::other("destination occupied"))}},
        |_|Err(io::Error::other("verification")), |_|panic!("cleanup after failed rollback"));
    let Outcome::Error(message)=result else{panic!("expected hard error")};
    assert!(message.contains("private-metadata") && message.contains("dest") && message.contains("source"));
    assert!(f.path("private-metadata").exists());
}
#[test]
fn cleanup_failure_is_reported() {
    let f=Fixture::new(); let mut meta=f.metadata();
    let result=transact(&mut meta,&f.path("private-metadata"),&f.path("dest"),&f.path("source"),&AtomicBool::new(true),None,
        |_|panic!("cancel"), |_|panic!("cancel"), |_|Err(io::Error::other("injected cleanup")));
    let Outcome::Error(message)=result else{panic!("silent cleanup failure")}; assert!(message.contains("cleanup failed"));
}
#[test]
fn cancellation_after_move_rolls_back() {
    let f=Fixture::new(); let mut meta=f.metadata(); let cancel=AtomicBool::new(false); let mut targets=Vec::new();
    let result=transact(&mut meta,&f.path("private-metadata"),&f.path("dest"),&f.path("source"),&cancel,None,
        |path|{targets.push(path.to_path_buf());Ok(())}, |_|{cancel.store(true,Ordering::Relaxed);Ok(())},delete_metadata);
    assert!(matches!(result,Outcome::Cancelled)); assert_eq!(targets.len(),2);
}
#[test]
fn verified_success_never_rolls_back_or_cleans() {
    let f=Fixture::new(); let mut meta=f.metadata(); let mut moves=0;
    let result=transact(&mut meta,&f.path("private-metadata"),&f.path("dest"),&f.path("source"),&AtomicBool::new(false),None,
        |_|{moves+=1;Ok(())}, |_|Ok(()), |_|panic!("cleanup successful metadata"));
    assert!(matches!(result,Outcome::Moved)); assert_eq!(moves,1);
}

#[test]
fn actual_failed_verification_restores_data_and_removes_metadata() {
    let f=Fixture::new(); let original=f.path("source"); let dest=f.path("payload");
    fs::create_dir(&original).unwrap(); fs::write(original.join("test"),b"unchanged").unwrap();
    let source=open_directory(&original,true).unwrap(); let mut meta=f.metadata();
    let result=transact(&mut meta,&f.path("private-metadata"),&dest,&original,&AtomicBool::new(false),Some(identity(&source).unwrap()),
        |target|rename_handle(&source,target), |_|Err(io::Error::other("injected verification")),delete_metadata);
    assert!(matches!(result,Outcome::Error(_))); drop(meta); drop(source);
    assert_eq!(fs::read(original.join("test")).unwrap(),b"unchanged");
    assert!(!dest.exists()); assert!(!f.path("private-metadata").exists());
}
#[test]
fn known_failed_move_cleans_metadata_and_preserves_collision() {
    let f=Fixture::new(); let original=f.path("source"); let dest=f.path("collision");
    fs::create_dir(&original).unwrap(); fs::create_dir(&dest).unwrap();
    fs::write(original.join("original"),b"original").unwrap(); fs::write(dest.join("unrelated"),b"keep").unwrap();
    let source=open_directory(&original,true).unwrap(); let mut meta=f.metadata();
    let result=transact(&mut meta,&f.path("private-metadata"),&dest,&original,&AtomicBool::new(false),Some(identity(&source).unwrap()),
        |target|rename_handle(&source,target), |_|panic!("verify failed move"),delete_metadata);
    assert!(matches!(result,Outcome::Error(_))); drop(meta); drop(source);
    assert!(!f.path("private-metadata").exists());
    assert_eq!(fs::read(original.join("original")).unwrap(),b"original"); assert_eq!(fs::read(dest.join("unrelated")).unwrap(),b"keep");
}
#[test]
fn actual_rollback_collision_preserves_original_and_unrelated_data() {
    let f=Fixture::new(); let original=f.path("source"); let dest=f.path("payload");
    fs::create_dir(&original).unwrap(); fs::write(original.join("original"),b"original").unwrap();
    let source=open_directory(&original,true).unwrap(); let mut meta=f.metadata();
    let result=transact(&mut meta,&f.path("private-metadata"),&dest,&original,&AtomicBool::new(false),None,
        |target|rename_handle(&source,target), |_|{
            fs::create_dir(&original)?; fs::write(original.join("unrelated"),b"unrelated")?;
            Err(io::Error::other("injected verification"))
        },delete_metadata);
    assert!(matches!(result,Outcome::Error(_))); drop(meta); drop(source);
    assert_eq!(fs::read(dest.join("original")).unwrap(),b"original");
    assert_eq!(fs::read(original.join("unrelated")).unwrap(),b"unrelated");
    assert!(f.path("private-metadata").exists());
}
#[test]
fn metadata_write_failure_cleans_only_owned_file() {
    let f=Fixture::new(); fs::write(f.path("owned"),b"").unwrap(); fs::write(f.path("unrelated"),b"keep").unwrap();
    let mut file=OpenOptions::new().access_mode(1 | DELETE_ACCESS).share_mode(1).open(f.path("owned")).unwrap();
    assert!(file.write_all(b"no write access").is_err());
    let result=cleanup_outcome(&file,&f.path("owned"),Outcome::Error("write failure".into()));
    assert!(matches!(result,Outcome::Error(_))); drop(file);
    assert!(!f.path("owned").exists()); assert_eq!(fs::read(f.path("unrelated")).unwrap(),b"keep");
}
#[test]
fn junction_and_redirected_ancestor_require_shell() {
    let f=Fixture::new(); fs::create_dir(f.path("scan")).unwrap(); fs::create_dir_all(f.path("target/child")).unwrap();
    fs::write(f.path("target/precious"),b"keep").unwrap(); let junction=f.path("scan").join("junction");
    let output=std::process::Command::new("cmd.exe").args(["/d","/c","mklink","/J"])
        .arg(&junction).arg(f.path("target")).output().unwrap();
    assert!(output.status.success(),"could not create disposable junction: {} {}",
        String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    let scan=folder_size(&f.path("scan"),&AtomicBool::new(false));
    let ancestor=pin_chain(&junction.join("child"));
    fs::remove_dir(&junction).unwrap();
    assert!(scan.is_err()); assert!(ancestor.is_err());
    assert_eq!(fs::read(f.path("target/precious")).unwrap(),b"keep");
}
#[test]
fn offline_file_requires_shell_without_opening_contents() {
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW,FILE_FLAGS_AND_ATTRIBUTES};
    let f=Fixture::new(); let file=f.path("offline"); fs::write(&file,b"keep").unwrap();
    unsafe{SetFileAttributesW(&HSTRING::from(file.as_os_str()),FILE_FLAGS_AND_ATTRIBUTES(0x1000)).unwrap();}
    let result=folder_size(&f.0,&AtomicBool::new(false));
    unsafe{SetFileAttributesW(&HSTRING::from(file.as_os_str()),FILE_FLAGS_AND_ATTRIBUTES(0x80)).unwrap();}
    assert!(result.is_err()); assert_eq!(fs::read(file).unwrap(),b"keep");
}

// Explicit component benchmark: private fixtures only, no Shell mutation or quota query.
// It cannot measure end-to-end recycle latency or establish a benefit over Shell.
#[test]
#[ignore = "explicit private-fixture component benchmark"]
fn benchmark_removed_bin_walk_and_target_verification() {
    let f=Fixture::new(); let original=f.path("source"); let dest=f.path("payload"); fs::create_dir(&original).unwrap();
    let source=open_directory(&original,true).unwrap(); let id=identity(&source).unwrap();
    let record=metadata_record(&original,0,1).unwrap(); let mut meta=f.metadata(); meta.write_all(&record).unwrap();
    let private_bin=f.path("simulated-bin"); fs::create_dir(&private_bin).unwrap();
    for i in 0..536 {fs::write(private_bin.join(format!("$I{i:06X}")),&record).unwrap();}
    rename_handle(&source,&dest).unwrap();
    let mut old=Vec::new();let mut new=Vec::new();
    for _ in 0..31 {
        let start=Instant::now();
        for entry in fs::read_dir(&private_bin).unwrap(){let bytes=fs::read(entry.unwrap().path()).unwrap();assert_eq!(&bytes[..8],&2u64.to_le_bytes());}
        old.push(start.elapsed().as_micros());
        let start=Instant::now();verify_pair(&mut meta,&record,&source,id,&original,&dest).unwrap();new.push(start.elapsed().as_micros());
    }
    old.sort();new.sort();
    println!("COMPONENT_ONLY records=536 runs=31 old_format_walk_median_us={} new_specific_pair_median_us={} old_reads=536 new_reads=1 excludes_shell_recognition=true",old[15],new[15]);
    rename_handle(&source,&original).unwrap();
}
