//! Opt-in, same-volume folder recycling. No fallback after a move is attempted.
//! The on-disk format is not a supported Windows API: real Restore compatibility
//! must be confirmed by controlled integration tests before default activation.
#![allow(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions}, io::{self, Read, Seek, SeekFrom, Write},
    os::windows::{ffi::OsStrExt, fs::{MetadataExt, OpenOptionsExt}, io::{AsRawHandle, FromRawHandle}},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use windows::{core::HSTRING, Win32::{
    Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
    Security::{Authorization::ConvertSidToStringSidW, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER},
    Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW, GetVolumeNameForVolumeMountPointW,
        GetFileInformationByHandle, GetFinalPathNameByHandleW, SetFileInformationByHandle,
        BY_HANDLE_FILE_INFORMATION, FILE_NAME_OPENED, GETFINALPATHNAMEBYHANDLE_FLAGS, VOLUME_NAME_GUID,
        FILE_RENAME_INFO, FILE_DISPOSITION_INFO, FileRenameInfo, FileDispositionInfo,
        ReOpenFile, FILE_SHARE_MODE, FILE_FLAGS_AND_ATTRIBUTES},
    System::{Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD},
        Threading::{GetCurrentProcess, OpenProcessToken}},
    UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO, SHChangeNotify, SHCNE_RMDIR, SHCNE_UPDATEDIR,
        SHCNF_FLUSHNOWAIT, SHCNF_PATHW},
}};

const UNSAFE_ATTRIBUTES: u32 = 0x400 | 0x1000 | 0x40000 | 0x400000; // reparse, offline, recall
const DIRECTORY_FLAGS: u32 = 0x02000000 | 0x00200000; // backup semantics, open reparse point
const READ_ATTRIBUTES: u32 = 0x80;
const DELETE_ACCESS: u32 = 0x10000;
static COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) enum Outcome { Moved, Fallback, Cancelled, Error(String) }
pub(super) fn enabled() -> bool {
    // Enabled for eligible local NTFS folders by default.
    // FLDR_FAST_RECYCLE=0 forces the Windows Shell implementation.
    std::env::var_os("FLDR_FAST_RECYCLE").as_deref()
        != Some(std::ffi::OsStr::new("0"))
}

// Path-free stage records, including failed/early-return stages. No work when tracing is off
// except one clock read per stage. Existing total-duration routing trace remains unchanged.
struct Stage { name: &'static str, started: Instant }
impl Stage { fn new(name: &'static str) -> Self { Self { name, started: Instant::now() } } }
impl Drop for Stage {
    fn drop(&mut self) {
        if std::env::var_os("FLDR_FAST_RECYCLE_TRACE").as_deref() != Some(std::ffi::OsStr::new("1")) { return; }
        if let Ok(mut log) = OpenOptions::new().append(true).create(true)
            .open(std::env::temp_dir().join("FLDR-Fast-Recycle-Trace.log")) {
            let _ = writeln!(log, "stage={} elapsed_us={} thread={:?}", self.name,
                self.started.elapsed().as_micros(), std::thread::current().id());
        }
    }
}

pub(super) fn try_folder(path: &Path, cancel: &AtomicBool) -> Outcome {
    if cancel.load(Ordering::Relaxed) { return Outcome::Cancelled; }
    let eligibility = Stage::new("eligibility");
    let Some(root) = drive_root(path) else { return Outcome::Fallback; };
    if !is_fixed_ntfs(&root) { return Outcome::Fallback; }
    // Deny rename/delete of every source ancestor while resolving child paths.
    let Ok(_source_parents) = pin_chain(path.parent().unwrap()) else { return Outcome::Fallback; };
    let Ok(source) = open_directory(path, true) else { return Outcome::Fallback; };
    if !has_local_volume_guid(&source) { return Outcome::Fallback; }
    let Ok(source_id) = identity(&source) else { return Outcome::Fallback; };
    drop(eligibility);

    let configuration = Stage::new("configuration");
    if !policy_allows() { return Outcome::Fallback; }
    let Some(sid) = current_sid() else { return Outcome::Fallback; };
    let bin = root.join("$Recycle.Bin").join(sid);
    let Ok(_bin_pins) = pin_chain(&bin) else { return Outcome::Fallback; };
    let Some(limit) = bin_limit_bytes(&root) else { return Outcome::Fallback; };
    drop(configuration);
    let capacity = Stage::new("capacity_query");
    let Some(used) = bin_bytes(&root) else { return Outcome::Fallback; };
    drop(capacity);
    let scan = Stage::new("size_scan");
    let size = match folder_size(path, cancel) {
        Ok(size) => size,
        Err(_) if cancel.load(Ordering::Relaxed) => return Outcome::Cancelled,
        Err(_) => return Outcome::Fallback,
    };
    drop(scan);
    if used.checked_add(size).is_none_or(|total| total > limit) { return Outcome::Fallback; }
    if cancel.load(Ordering::Relaxed) { return Outcome::Cancelled; }
    let prepare = Stage::new("metadata_prepare");
    let Some(ticks) = SystemTime::now().duration_since(UNIX_EPOCH).ok()
        .and_then(|time| time.as_nanos().checked_div(100))
        .and_then(|ticks| ticks.checked_add(116_444_736_000_000_000))
        .and_then(|ticks| u64::try_from(ticks).ok()) else { return Outcome::Fallback; };
    let Ok(record) = metadata_record(path, size, ticks) else { return Outcome::Fallback; };
    // Preserve non-Unicode extensions exactly, rather than replacing their code units.
    let extension = path.extension().map(|ext| {
        let mut text = std::ffi::OsString::from("."); text.push(ext); text
    }).unwrap_or_default();
    drop(prepare);
    for _ in 0..64 {
        if cancel.load(Ordering::Relaxed) { return Outcome::Cancelled; }
        let suffix = suffix(ticks, std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
        let mut iname = std::ffi::OsString::from(format!("$I{suffix}")); iname.push(&extension);
        let mut rname = std::ffi::OsString::from(format!("$R{suffix}")); rname.push(&extension);
        let info = bin.join(iname);
        let dest = bin.join(rname);
        match fs::symlink_metadata(&dest) {
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(_) => return Outcome::Fallback,
        }
        // Exclusive creation and no delete/write sharing protect this exact metadata object.
        let mut metadata = match OpenOptions::new().read(true).write(true).create_new(true)
            .access_mode(0x80000000 | 0x40000000 | DELETE_ACCESS).share_mode(1).open(&info) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Outcome::Fallback,
        };
        let write = Stage::new("metadata_write_flush");
        let written = metadata.write_all(&record).and_then(|_| metadata.sync_all());
        drop(write);
        if let Err(error) = written {
            return cleanup_outcome(&metadata, &info, Outcome::Error(format!("Recycle metadata write failed: {error}")));
        }
        let publish = Stage::new("metadata_reader_handoff");
        // A writer/DELETE handle is incompatible with Shell readers that deny
        // write/delete sharing, even though our writer specifies SHARE_READ.
        let guard = match metadata_read_guard(&mut metadata, &info) {
            Ok(guard) => guard,
            Err(error) => return cleanup_outcome(&metadata, &info,
                Outcome::Error(format!("Recycle metadata reader handoff failed: {error}"))),
        };
        drop(publish);
        // Synchronous Shell namespace enumeration cannot be a commit precondition:
        // the retained DELETE-capable source handle can prevent Shell readers from
        // seeing this item before the transaction closes those handles. Verify
        // the exact owned metadata bytes, payload object identity and old-path
        // absence here. Controlled Explorer Restore tests qualify compatibility.
        let verify = |file: &mut File| -> io::Result<()> {
            let _stage = Stage::new("specific_item_verification");
            let _pair = Stage::new("pair_verification");
            verify_pair(file, &record, &source, source_id, path, &dest)
        };
        let rename = |target: &Path| {
            let _stage = Stage::new("handle_rename"); rename_handle(&source, target)
        };
        let result = transact(&mut metadata, &info, &dest, path, cancel, Some(source_id),
            rename, verify, |file| guard.cleanup(file));
        // Release the DELETE-capable source handle and metadata readers before
        // notifying Windows Shell about the completed transaction. Previously the
        // Shell lookup ran while these handles were still retained.
        drop(source);
        drop(guard);
        drop(metadata);
        if matches!(result, Outcome::Moved) {
            let _stage = Stage::new("notifications");
            notify(SHCNE_RMDIR, path); notify(SHCNE_UPDATEDIR, &bin);
        } else {
            notify(SHCNE_UPDATEDIR, &bin);
        }
        return result;
    }
    Outcome::Fallback
}

fn metadata_record(path: &Path, size: u64, ticks: u64) -> io::Result<Vec<u8>> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) { return Err(io::Error::other("NUL in original path")); }
    let length = u32::try_from(wide.len() + 1).map_err(io::Error::other)?;
    let mut bytes = Vec::with_capacity(28 + length as usize * 2);
    bytes.extend_from_slice(&2u64.to_le_bytes()); bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&ticks.to_le_bytes()); bytes.extend_from_slice(&length.to_le_bytes());
    for unit in wide.into_iter().chain(Some(0)) { bytes.extend_from_slice(&unit.to_le_bytes()); }
    Ok(bytes)
}

fn suffix(ticks: u64, pid: u32, sequence: u64) -> String {
    // Mix *before* truncation. Reservations, not probabilistic uniqueness, ensure safety.
    let mut value = ticks ^ (u64::from(pid) << 32) ^ sequence.wrapping_mul(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    format!("{:06X}", (value ^ (value >> 31)) & 0xffffff)
}

// Only conventional absolute DOS paths are eligible. Extended/device/UNC paths use Shell.
fn drive_root(path: &Path) -> Option<PathBuf> {
    let text = path.as_os_str().to_str()?;
    let raw: Vec<_> = text.split(['\\', '/']).collect();
    if raw.iter().skip(1).any(|part| part.is_empty() || *part == "." || *part == ".."
        || part.ends_with(['.', ' ']) || part.contains(':')) { return None; }
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else { return None; };
    let std::path::Prefix::Disk(letter) = prefix.kind() else { return None; };
    if components.next()? != Component::RootDir { return None; }
    let mut count = 0;
    for component in components {
        let Component::Normal(name) = component else { return None; };
        let name = name.to_str()?;
        if name.eq_ignore_ascii_case("$Recycle.Bin") || name.eq_ignore_ascii_case("System Volume Information") { return None; }
        count += 1;
    }
    (count > 0).then(|| PathBuf::from(format!("{}:\\", char::from(letter).to_ascii_uppercase())))
}

fn open_directory(path: &Path, movable: bool) -> io::Result<File> {
    // FILE_LIST_DIRECTORY makes Windows enforce deny-delete sharing. Attribute-only
    // handles do not reliably prevent directory replacement.
    let file = OpenOptions::new().access_mode(READ_ATTRIBUTES | 1 | if movable { DELETE_ACCESS } else { 0 })
        .share_mode(1 | 2).custom_flags(DIRECTORY_FLAGS).open(path)?;
    let meta = file.metadata()?;
    if !meta.is_dir() || meta.file_attributes() & UNSAFE_ATTRIBUTES != 0 {
        return Err(io::Error::other("Unsafe directory attributes"));
    }
    Ok(file)
}

fn pin_chain(path: &Path) -> io::Result<Vec<File>> {
    let mut paths: Vec<_> = path.ancestors().collect(); paths.reverse();
    paths.into_iter().map(|part| open_directory(part, false)).collect()
}

type Identity = (u32, u32, u32);

fn has_local_volume_guid(file: &File) -> bool {
    // Network shares have no volume GUID. Require proof on the opened handle
    // as well as the earlier DOS drive-type check before touching the bin.
    const FLAGS: GETFINALPATHNAMEBYHANDLE_FLAGS =
        GETFINALPATHNAMEBYHANDLE_FLAGS(VOLUME_NAME_GUID.0 | FILE_NAME_OPENED.0);
    let handle = HANDLE(file.as_raw_handle());
    let mut name = [0u16; 512];
    let len = unsafe { GetFinalPathNameByHandleW(handle, &mut name, FLAGS) } as usize;
    let volume_prefix = windows::core::w!(r"\\?\Volume{");
    let prefix = unsafe { volume_prefix.as_wide() };
    if len < name.len() {
        return name[..len].starts_with(prefix);
    }
    let mut name = vec![0u16; len];
    let len = unsafe { GetFinalPathNameByHandleW(handle, &mut name, FLAGS) } as usize;
    len < name.len() && name[..len].starts_with(prefix)
}

fn identity(file: &File) -> io::Result<Identity> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.map_err(io::Error::other)?;
    Ok((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

fn rename_handle(file: &File, target: &Path) -> io::Result<()> {
    let name: Vec<u16> = target.as_os_str().encode_wide().collect();
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    // FileName is NUL-terminated; FileNameLength excludes the terminator.
    let length = (offset + (name.len() + 1) * 2).max(std::mem::size_of::<FILE_RENAME_INFO>());
    let mut storage = vec![0u64; length.div_ceil(8)];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        // Zeroed ReplaceIfExists is FALSE. No copy fallback and no replacement.
        (*info).FileNameLength = u32::try_from(name.len() * 2).map_err(io::Error::other)?;
        std::ptr::copy_nonoverlapping(name.as_ptr(), storage.as_mut_ptr().cast::<u8>().add(offset).cast(), name.len());
        SetFileInformationByHandle(HANDLE(file.as_raw_handle()), FileRenameInfo,
            storage.as_ptr().cast(), u32::try_from(length).map_err(io::Error::other)?)
    }.map_err(io::Error::other)
}

fn delete_metadata(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true.into() };
    let dispose = |file: &File| unsafe {
        SetFileInformationByHandle(HANDLE(file.as_raw_handle()), FileDispositionInfo,
            (&raw const info).cast(), std::mem::size_of_val(&info) as u32)
    }.map_err(io::Error::other);
    if let Ok(()) = dispose(file) { return Ok(()); }
    // Published metadata has a read-only anchor. Reopen the SAME object for
    // cleanup, never its pathname, after releasing the deny-delete read guard.
    dispose(&reopen_metadata(file, DELETE_ACCESS, 1 | 2 | 4)?)
}

fn reopen_metadata(file: &File, access: u32, sharing: u32) -> io::Result<File> {
    let handle = unsafe { ReOpenFile(HANDLE(file.as_raw_handle()), access,
        FILE_SHARE_MODE(sharing), FILE_FLAGS_AND_ATTRIBUTES(0x00200000)) }.map_err(io::Error::other)?;
    Ok(unsafe { File::from_raw_handle(handle.0) })
}

struct MetadataReadGuard(std::cell::RefCell<Option<File>>);
impl MetadataReadGuard {
    fn cleanup(&self, file: &File) -> io::Result<()> {
        drop(self.0.borrow_mut().take());
        delete_metadata(file)
    }
}

fn metadata_read_guard(metadata: &mut File, path: &Path) -> io::Result<MetadataReadGuard> {
    let expected_id = identity(metadata)?;
    // Keep an object-identity anchor across the writer->reader handoff. ReOpenFile
    // reopens that object even if its name changes; DuplicateHandle would retain
    // the writer's file-object sharing state and would not fix this conflict.
    let anchor = reopen_metadata(metadata, 0x80000000, 1 | 2 | 4)?;
    drop(std::mem::replace(metadata, anchor));
    let guard = reopen_metadata(metadata, 0x80000000, 1)?;
    // A brief handoff window allowed rename/write access. Do not move the source
    // unless this exact metadata object still occupies its reserved pathname.
    if identity(&guard)? != expected_id || path_identity(path)? != expected_id
        || guard.metadata()?.file_attributes() & UNSAFE_ATTRIBUTES != 0 {
        return Err(io::Error::other("Reserved metadata identity/path changed during reader handoff"));
    }
    Ok(MetadataReadGuard(std::cell::RefCell::new(Some(guard))))
}

fn cleanup_outcome(file: &File, info: &Path, outcome: Outcome) -> Outcome {
    match delete_metadata(file) {
        Ok(()) => outcome,
        Err(error) => Outcome::Error(format!("Recycle metadata cleanup failed; retained {}: {error}", info.display())),
    }
}

// Injectable transitions allow fault tests without a live Recycle Bin. Never fall back
// after attempting the move: even a returned error must not cause a second deletion.
fn transact(
    metadata: &mut File, info: &Path, dest: &Path, original: &Path, cancel: &AtomicBool,
    known_source: Option<Identity>,
    mut rename: impl FnMut(&Path) -> io::Result<()>,
    mut verify: impl FnMut(&mut File) -> io::Result<()>,
    mut cleanup: impl FnMut(&File) -> io::Result<()>,
) -> Outcome {
    let cleanup_result = |cleanup: &mut dyn FnMut(&File) -> io::Result<()>, file: &File, result| {
        match cleanup(file) {
            Ok(()) => result,
            Err(error) => Outcome::Error(format!("Recycle metadata cleanup failed; retained {}: {error}", info.display())),
        }
    };
    if cancel.load(Ordering::Relaxed) { return cleanup_result(&mut cleanup, metadata, Outcome::Cancelled); }
    if let Err(error) = rename(dest) {
        // The retained source handle denies outside rename/delete. If that exact
        // object is still at the original path, only our metadata needs cleanup.
        if known_source.is_some_and(|expected| path_identity(original).ok() == Some(expected)) {
            return cleanup_result(&mut cleanup, metadata,
                Outcome::Error(format!("Native recycle move failed; source unchanged: {error}")));
        }
        // Do not discard recovery metadata when a mutation returned an ambiguous failure.
        return Outcome::Error(format!("Native recycle move failed: {error}. Retained metadata: {}; payload destination: {}; original: {}",
            info.display(), dest.display(), original.display()));
    }
    let verification = verify(metadata);
    if verification.is_ok() && !cancel.load(Ordering::Relaxed) { return Outcome::Moved; }
    let _stage = Stage::new("rollback_cleanup");
    if let Err(error) = rename(original) {
        // Keep BOTH objects; never discard recovery information after a failed rollback.
        return Outcome::Error(format!("Recycle verification/cancellation rollback failed: {error}. Payload: {}; metadata: {}; original: {}",
            dest.display(), info.display(), original.display()));
    }
    if known_source.is_some_and(|expected| path_identity(original).ok() != Some(expected)) {
        return Outcome::Error(format!("Rollback could not be verified. Retained metadata: {}; payload destination: {}; original: {}",
            info.display(), dest.display(), original.display()));
    }
    let outcome = if cancel.load(Ordering::Relaxed) { Outcome::Cancelled }
        else { Outcome::Error(format!("Native recycle verification failed; folder returned to original path: {}",
            verification.unwrap_err())) };
    cleanup_result(&mut cleanup, metadata, outcome)
}

fn path_identity(path: &Path) -> io::Result<Identity> {
    let file = OpenOptions::new().access_mode(READ_ATTRIBUTES).share_mode(1 | 2 | 4)
        .custom_flags(DIRECTORY_FLAGS).open(path)?;
    identity(&file)
}

fn verify_pair(metadata: &mut File, expected: &[u8], source: &File, id: Identity, original: &Path, dest: &Path) -> io::Result<()> {
    if metadata.metadata()?.len() != expected.len() as u64 { return Err(io::Error::other("Metadata length changed")); }
    metadata.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![0; expected.len()]; metadata.read_exact(&mut bytes)?;
    if bytes != expected { return Err(io::Error::other("Metadata content changed")); }
    // FILE_SHARE_DELETE lets this reader coexist with our retained DELETE-access handle.
    let payload = OpenOptions::new().access_mode(READ_ATTRIBUTES).share_mode(1 | 2 | 4)
        .custom_flags(DIRECTORY_FLAGS).open(dest)?;
    if identity(&payload)? != id || identity(source)? != id {
        return Err(io::Error::other("Payload identity changed"));
    }
    match fs::symlink_metadata(original) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err(io::Error::other("Original path no longer absent")),
    }
}

fn folder_size(root: &Path, cancel: &AtomicBool) -> io::Result<u64> {
    let mut size = 0u64;
    let root_pin = OpenOptions::new().access_mode(READ_ATTRIBUTES).share_mode(1 | 2 | 4)
        .custom_flags(DIRECTORY_FLAGS).open(root)?;
    let mut stack = vec![(root.to_path_buf(), root_pin)];
    let mut pins = Vec::new();
    while let Some((dir, pinned)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) { return Err(io::Error::other("Cancelled")); }
        let meta = pinned.metadata()?;
        if !meta.is_dir() || meta.file_attributes() & UNSAFE_ATTRIBUTES != 0 { return Err(io::Error::other("Unsafe traversal")); }
        // Keep ancestors pinned until traversal ends; queued paths cannot be redirected.
        pins.push(pinned);
        for entry in fs::read_dir(&dir)? {
            if cancel.load(Ordering::Relaxed) { return Err(io::Error::other("Cancelled")); }
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_attributes() & UNSAFE_ATTRIBUTES != 0 { return Err(io::Error::other("Reparse/cloud child requires Shell")); }
            if meta.is_dir() {
                // Bound handle retention. Large directory trees can use the Shell;
                // never exhaust the process handle table to establish eligibility.
                if pins.len() + stack.len() >= 4096 { return Err(io::Error::other("Directory pin budget requires Shell")); }
                let path = entry.path();
                let child = open_directory(&path, false)?;
                stack.push((path, child));
            } else {
                size = size.checked_add(meta.len()).ok_or_else(|| io::Error::other("Size overflow"))?;
            }
        }
    }
    Ok(size)
}

fn is_fixed_ntfs(root: &Path) -> bool {
    let root = HSTRING::from(root.as_os_str()); let mut buffer = [0u16; 32];
    (unsafe { GetDriveTypeW(&root) == 3 && GetVolumeInformationW(&root, None, None, None, None, Some(&mut buffer)).is_ok() })
        && String::from_utf16_lossy(&buffer).trim_end_matches('\0').eq_ignore_ascii_case("NTFS")
}

// A missing value is different from an unreadable or wrongly typed policy value.
fn reg_dword(hive: HKEY, key: &str, name: &str) -> io::Result<Option<u32>> {
    let mut value = 0u32; let mut size = 4u32;
    let status = unsafe { RegGetValueW(hive, &HSTRING::from(key), &HSTRING::from(name), RRF_RT_REG_DWORD,
        None, Some((&raw mut value).cast()), Some(&mut size)) };
    match status.0 {
        0 if size == 4 => Ok(Some(value)),
        2 | 3 => Ok(None),
        code => Err(io::Error::from_raw_os_error(code as i32)),
    }
}
fn policy_allows() -> bool {
    let key = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().all(|hive| {
        matches!(reg_dword(hive, key, "NoRecycleFiles"), Ok(None | Some(0)))
            && matches!(reg_dword(hive, key, "RecycleBinSize"), Ok(None))
    })
}
fn bin_limit_bytes(root: &Path) -> Option<u64> {
    let mut volume = [0u16; 64];
    unsafe { GetVolumeNameForVolumeMountPointW(&HSTRING::from(root.as_os_str()), &mut volume) }.ok()?;
    let volume = String::from_utf16_lossy(&volume);
    let guid = volume.get(volume.find('{')?..=volume.find('}')?)?;
    let key = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}");
    if !matches!(reg_dword(HKEY_CURRENT_USER, &key, "NukeOnDelete"), Ok(None | Some(0))) { return None; }
    Some(u64::from(reg_dword(HKEY_CURRENT_USER, &key, "MaxCapacity").ok()??) * 1024 * 1024)
}
fn bin_bytes(root: &Path) -> Option<u64> {
    let mut info = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, ..Default::default() };
    unsafe { SHQueryRecycleBinW(&HSTRING::from(root.as_os_str()), &raw mut info) }.ok()?;
    u64::try_from(info.i64Size).ok()
}
fn current_sid() -> Option<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }.ok()?;
    let mut len = 0u32;
    unsafe { let _ = GetTokenInformation(token, TokenUser, None, 0, &raw mut len); }
    if len == 0 { unsafe { let _ = CloseHandle(token); } return None; }
    let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
    let got = unsafe { GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), len, &raw mut len) };
    unsafe { let _ = CloseHandle(token); } got.ok()?;
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = windows::core::PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &raw mut text) }.ok()?;
    let result = unsafe { text.to_string().ok() };
    unsafe { let _ = LocalFree(Some(HLOCAL(text.0.cast()))); } result
}

fn notify(event: windows::Win32::UI::Shell::SHCNE_ID, path: &Path) {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { SHChangeNotify(event, SHCNF_PATHW | SHCNF_FLUSHNOWAIT, Some(wide.as_ptr().cast()), None); }
}

#[cfg(test)]
#[path = "fast_recycle_tests.rs"]
mod tests;
