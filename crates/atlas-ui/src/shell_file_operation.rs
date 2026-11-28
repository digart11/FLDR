#![allow(unsafe_code, non_snake_case)]
//! Native Windows operations. Run on blocking workers, never GPUI callbacks.
//! Each call owns an STA and an item progress sink, so cancellation, per-item
//! HRESULTs, and Windows-selected collision names are not mistaken for success.
use atlas_model::ops::{OpError, OpErrorKind, Resolution, TransferKind};
#[path = "fast_recycle.rs"]
mod fast_recycle;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use windows::core::{implement, Ref, HRESULT, PCWSTR};
use windows::core::Interface as _;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::*;

/// Create a real Shell link without replacing a pre-existing destination.
pub(crate) fn create_shortcut(source: PathBuf, destination: PathBuf) -> Result<PathBuf, OpError> {
    let error_path = source.clone();
    std::thread::spawn(move || {
        let run = || -> Result<PathBuf, OpError> {
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }
                .map_err(|error| OpError { path: source.clone(), kind: error_kind(&error) })?;
            let _apartment = Apartment;
            let name = source.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_else(|| {
                // A copied drive root has no file_name; a literal `C:\` is not
                // a valid Windows filename for its link.
                let label: String = source.to_string_lossy().chars().filter(|ch| ch.is_alphanumeric()).collect();
                if label.is_empty() { "Drive".into() } else { label.into() }
            });
            let suffix = rust_i18n::t!("shortcut_filename_suffix");
            let mut counter = 1u32;
            let target = loop {
                let mut link_name = name.clone();
                link_name.push(format!(" - {suffix}"));
                if counter > 1 { link_name.push(format!(" ({counter})")); }
                link_name.push(".lnk");
                let path = destination.join(link_name);
                match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(file) => { drop(file); break path; }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        counter = counter.checked_add(1).ok_or_else(|| OpError {
                            path: path.clone(), kind: OpErrorKind::Msg("Too many shortcuts".into()) })?;
                    },
                    Err(error) => return Err(OpError { path, kind: OpErrorKind::Msg(error.to_string()) }),
                }
            };
            let save = || -> windows::core::Result<()> {
                unsafe {
                    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_ALL)?;
                    link.SetPath(PCWSTR(wide(source.as_os_str()).as_ptr()))?;
                    if let Some(parent) = source.parent() { link.SetWorkingDirectory(PCWSTR(wide(parent.as_os_str()).as_ptr()))?; }
                    let persistence: windows::Win32::System::Com::IPersistFile = link.cast()?;
                    persistence.Save(PCWSTR(wide(target.as_os_str()).as_ptr()), true)
                }
            };
            if let Err(error) = save() {
                let _ = std::fs::remove_file(&target);
                return Err(OpError { path: target, kind: error_kind(&error) });
            }
            Ok(target)
        };
        run()
    }).join().unwrap_or_else(|_| Err(OpError { path: error_path, kind: OpErrorKind::Msg("Windows shortcut worker failed".into()) }))
}

#[derive(Debug, Clone)]
pub(crate) enum Operation {
    Copy {
        source: PathBuf,
        destination: PathBuf,
        conflict: Resolution,
    },
    Move {
        source: PathBuf,
        destination: PathBuf,
        conflict: Resolution,
    },
    Rename {
        source: PathBuf,
        destination: PathBuf,
    },
    Delete {
        source: PathBuf,
        recycle: bool,
    },
}

impl Operation {
    fn source(&self) -> &Path {
        match self {
            Self::Copy { source, .. }
            | Self::Move { source, .. }
            | Self::Rename { source, .. }
            | Self::Delete { source, .. } => source,
        }
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

// Parsing names use backslashes even when a Rust caller joined a component
// containing '/'. Keep UTF-16 intact, including non-Unicode Windows filenames.
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().map(|unit| if unit == b'/' as u16 { b'\\' as u16 } else { unit })
        .chain(Some(0)).collect()
}

fn shell_path(item: &IShellItem) -> windows::core::Result<PathBuf> {
    unsafe {
        let value = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = std::ffi::OsString::from_wide(value.as_wide());
        CoTaskMemFree(Some(value.0.cast()));
        Ok(PathBuf::from(path))
    }
}

#[derive(Default)]
struct ItemResult {
    result: Option<HRESULT>,
    failure: Option<HRESULT>,
    destination: Option<PathBuf>,
    failures: Vec<(PathBuf, HRESULT)>,
    skipped: Vec<PathBuf>,
}

#[implement(IFileOperationProgressSink)]
struct Progress {
    source: PathBuf,
    state: Arc<Mutex<ItemResult>>,
    cancel: Arc<AtomicBool>,
    /// Only the batch-wide advised sink forwards Shell work estimates.
    progress: Option<(u64, flume::Sender<atlas_model::app_state::BgResult>)>,
    skip_destination: Option<PathBuf>,
}
impl Progress {
    fn check(&self) -> windows::core::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(windows::core::Error::from_hresult(
                COPYENGINE_E_USER_CANCELLED,
            ))
        } else {
            Ok(())
        }
    }
    fn check_transfer(&self, original: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        self.check()?;
        let top_level = original.as_ref().and_then(|item| shell_path(item).ok())
            .is_some_and(|path| atlas_core::fs::transfer::paths_equal_ci(&path, &self.source));
        if top_level && self.skip_destination.as_ref().is_some_and(|path| path.symlink_metadata().is_ok()) {
            // Reject a raced-in target. An error is preferable to silently
            // overwriting it under the noninteractive fixture flags.
            return Err(windows::core::Error::from_hresult(HRESULT::from_win32(183)));
        }
        Ok(())
    }
    fn record(&self, result: HRESULT, original: Ref<'_, IShellItem>, item: Ref<'_, IShellItem>) {
        if let Some(mut state) = self.state.lock().or_else(atlas_model::sync::recover).ok() {
            if result.is_err() || shell_skipped(result) {
                state.failure.get_or_insert(result);
                let path = original.as_ref().and_then(|item| shell_path(item).ok())
                    .unwrap_or_else(|| self.source.clone());
                if shell_skipped(result) { state.skipped.push(path); }
                else { state.failures.push((path, result)); }
            }
            // Windows invokes the same sink for descendants. Keep the top-level
            // destination for Undo instead of the last copied child path.
            if original
                .as_ref()
                .and_then(|item| shell_path(item).ok())
                .as_deref()
                == Some(self.source.as_path())
            {
                state.result = Some(result);
                state.destination = item.as_ref().and_then(|item| shell_path(item).ok());
            }
        }
    }
}
impl IFileOperationProgressSink_Impl for Progress_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        self.check()
    }
    fn FinishOperations(&self, result: HRESULT) -> windows::core::Result<()> {
        if result.is_err() {
            let mut state = self.state.lock().unwrap_or_else(|poison| poison.into_inner());
            if state.failure.is_none_or(|previous| previous.is_ok()) {
                state.failure = Some(result);
            }
        }
        Ok(())
    }
    fn PreRenameItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _name: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.check()
    }
    fn PostRenameItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _name: &PCWSTR,
        result: HRESULT,
        new_item: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(result, _item, new_item);
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.check_transfer(_item)
    }
    fn PostMoveItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
        result: HRESULT,
        new_item: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(result, _item, new_item);
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.check_transfer(_item)
    }
    fn PostCopyItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
        result: HRESULT,
        new_item: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(result, _item, new_item);
        Ok(())
    }
    fn PreDeleteItem(&self, _flags: u32, _item: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        self.check()
    }
    fn PostDeleteItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        result: HRESULT,
        new_item: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(result, _item, new_item);
        Ok(())
    }
    fn PreNewItem(
        &self,
        _flags: u32,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.check()
    }
    fn PostNewItem(
        &self,
        _flags: u32,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
        _template: &PCWSTR,
        _attributes: u32,
        result: HRESULT,
        new_item: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(result, _folder, new_item);
        Ok(())
    }
    fn UpdateProgress(&self, total: u32, sofar: u32) -> windows::core::Result<()> {
        if let Some((id, tx)) = &self.progress {
            // These are Shell work points, never bytes or a file count.
            let _ = tx.send(atlas_model::app_state::BgResult::OpProgress {
                id: *id, done: sofar as u64, total: total as u64,
                bytes_done: 0, bytes_total: 0, current_file: PathBuf::new(),
            });
        }
        self.check()
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

fn shell_skipped(result: HRESULT) -> bool {
    result == COPYENGINE_S_USER_IGNORED
}

fn shell_completed(result: HRESULT) -> bool {
    matches!(result, HRESULT(0) | COPYENGINE_S_MERGE | COPYENGINE_S_COLLISIONRESOLVED
        | COPYENGINE_S_KEEP_BOTH | COPYENGINE_S_DONT_PROCESS_CHILDREN | COPYENGINE_S_ALREADY_DONE
        // Observed for successfully merged parent directories. Their children
        // still have to finish without a failure/skip, and a destination is required.
        | COPYENGINE_S_NOT_HANDLED)
}

fn error_kind(error: &windows::core::Error) -> OpErrorKind {
    match error.code().0 as u32 {
        0x800704C7 | 0x80270000 | 0x80270001 | 0x80004004 => OpErrorKind::Cancelled,
        code if code >> 16 == 0x8007 => atlas_core::fs::transfer::io_error_kind(
            &std::io::Error::from_raw_os_error((code & 0xffff) as i32),
        ),
        _ => OpErrorKind::Msg(error.to_string()),
    }
}

/// Windows supplies UAC UI and an Explorer undo record for operations that can
/// be undone. Permanent delete intentionally omits both recycle and undo flags.
/// `interactive=false` prohibits elevation/dialogs and is used only by fixtures.
pub(crate) fn execute(
    operation: Operation,
    owner: Option<isize>,
    cancel: Arc<AtomicBool>,
    interactive: bool,
) -> Result<Option<PathBuf>, OpError> {
    let source = operation.source().to_path_buf();
    // Tokio's pool can reuse a thread initialized as MTA by another subsystem.
    // A fresh thread guarantees STA ownership and keeps COM objects/apartment
    // teardown together; no shell interface crosses a thread boundary.
    std::thread::Builder::new()
        .name("atlas-shell-file-operation".into())
        .spawn(move || execute_sta(operation, owner, cancel, interactive))
        .map_err(|error| OpError {
            path: source.clone(),
            kind: OpErrorKind::Msg(error.to_string()),
        })?
        .join()
        .unwrap_or_else(|_| {
            Err(OpError {
                path: source,
                kind: OpErrorKind::Msg("Windows shell operation worker failed".into()),
            })
        })
}

fn execute_sta(
    operation: Operation,
    owner: Option<isize>,
    cancel: Arc<AtomicBool>,
    interactive: bool,
) -> Result<Option<PathBuf>, OpError> {
    let source = operation.source().to_path_buf();
    let result = (|| -> windows::core::Result<Option<PathBuf>> {
        if cancel.load(Ordering::Relaxed) {
            return Err(windows::core::Error::from_hresult(
                COPYENGINE_E_USER_CANCELLED,
            ));
        }
        // A fresh blocking thread can enter an STA; changed apartment mode is a
        // real error, never silently followed by an unbalanced CoUninitialize.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        let _apartment = Apartment;
        let operation_api: IFileOperation =
            unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL)? };
        let mut flags = FOF_NOCONFIRMMKDIR | FOFX_EARLYFAILURE;
        let private = atlas_core::vault::live::is_private(operation.source()) || match &operation {
            Operation::Copy { destination, .. } | Operation::Move { destination, .. } | Operation::Rename { destination, .. } => atlas_core::vault::live::is_private(destination),
            Operation::Delete { .. } => false,
        };
        if !private && !matches!(operation, Operation::Delete { recycle: false, .. }) {
            flags |= FOF_ALLOWUNDO | FOFX_ADDUNDORECORD;
        }
        if matches!(operation, Operation::Delete { recycle: true, .. }) {
            flags |= FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING;
        }
        if interactive {
            flags |= FOFX_SHOWELEVATIONPROMPT;
        } else {
            flags |= FOF_SILENT | FOF_NOERRORUI | FOF_NOCONFIRMATION;
        }
        match &operation {
            Operation::Copy {
                conflict: Resolution::Rename,
                ..
            }
            | Operation::Move {
                conflict: Resolution::Rename,
                ..
            } => flags |= FOF_RENAMEONCOLLISION,
            Operation::Copy {
                conflict: Resolution::Overwrite,
                ..
            }
            | Operation::Move {
                conflict: Resolution::Overwrite,
                ..
            } => flags |= FOF_NOCONFIRMATION,
            _ => {}
        }
        unsafe {
            operation_api.SetOperationFlags(flags)?;
            if let Some(owner) = owner {
                operation_api.SetOwnerWindow(HWND(owner as *mut _))?;
            }
        }
        let state = Arc::new(Mutex::new(ItemResult::default()));
        let sink: IFileOperationProgressSink = Progress {
            source: source.clone(),
            state: state.clone(),
            cancel,
            progress: None,
            skip_destination: None,
        }
        .into();
        let source_wide = wide_path(&source);
        let item: IShellItem =
            unsafe { SHCreateItemFromParsingName(PCWSTR(source_wide.as_ptr()), None)? };
        unsafe {
            let copying = matches!(operation, Operation::Copy { .. });
            match operation {
                Operation::Copy {
                    destination,
                    conflict,
                    ..
                }
                | Operation::Move {
                    destination,
                    conflict,
                    ..
                } => {
                    if conflict == Resolution::Skip && destination.exists() {
                        return Err(windows::core::Error::from_hresult(HRESULT::from_win32(183)));
                    }
                    let parent = destination.parent().ok_or_else(|| {
                        windows::core::Error::from_hresult(HRESULT::from_win32(3))
                    })?;
                    let folder_wide = wide_path(parent);
                    let folder: IShellItem =
                        SHCreateItemFromParsingName(PCWSTR(folder_wide.as_ptr()), None)?;
                    let name = wide(destination.file_name().unwrap_or_default());
                    if copying {
                        operation_api.CopyItem(&item, &folder, PCWSTR(name.as_ptr()), &sink)?;
                    } else {
                        operation_api.MoveItem(&item, &folder, PCWSTR(name.as_ptr()), &sink)?;
                    }
                }
                Operation::Rename { destination, .. } => {
                    if source.parent() != destination.parent() {
                        return Err(windows::core::Error::from_hresult(HRESULT::from_win32(87)));
                    }
                    let name = wide(destination.file_name().unwrap_or_default());
                    operation_api.RenameItem(&item, PCWSTR(name.as_ptr()), &sink)?;
                }
                Operation::Delete { .. } => operation_api.DeleteItem(&item, &sink)?,
            }
            operation_api.PerformOperations()?;
            if operation_api.GetAnyOperationsAborted()?.as_bool() {
                return Err(windows::core::Error::from_hresult(
                    COPYENGINE_E_USER_CANCELLED,
                ));
            }
        }
        let state = state
            .lock()
            .map_err(|_| windows::core::Error::from_hresult(HRESULT::from_win32(1)))?;
        let result = state
            .failure
            .or(state.result)
            .ok_or_else(|| windows::core::Error::from_hresult(HRESULT::from_win32(1)))?;
        result.ok()?;
        if shell_skipped(result) {
            return Err(windows::core::Error::from_hresult(
                COPYENGINE_E_USER_CANCELLED,
            ));
        }
        Ok(state.destination.clone())
    })();
    result.map_err(|error| OpError {
        path: source,
        kind: error_kind(&error),
    })
}

pub(crate) fn transfer_operation(
    pair: &atlas_core::fs::transfer::TransferPair,
    kind: TransferKind,
) -> Operation {
    if kind == TransferKind::Copy {
        Operation::Copy {
            source: pair.src.clone(),
            destination: pair.dst.clone(),
            conflict: pair.on_conflict,
        }
    } else {
        Operation::Move {
            source: pair.src.clone(),
            destination: pair.dst.clone(),
            conflict: pair.on_conflict,
        }
    }
}

/// Native planning deliberately does not call the preserved engine's recursive
/// inspection, space scan, conflict dialog, publication or ownership helpers.
#[derive(Debug, Clone)]
pub(crate) struct NativeTransfer {
    pub pair: atlas_core::fs::transfer::TransferPair,
    /// Ask uses Shell conflict UI; resolved callers retain their explicit choice.
    pub ask: bool,
}

pub(crate) fn plan_transfer(req: &atlas_model::ops::TransferRequest) -> Result<Vec<NativeTransfer>, OpError> {
    use atlas_model::ops::ConflictPolicy;
    let mut conflict_index = 0;
    req.sources.iter().enumerate().map(|(index, source)| {
        let name = req.target_names.as_ref().and_then(|names| names.get(index))
            .filter(|name| !name.is_empty()).map(|name| name.as_os_str())
            .or_else(|| source.file_name()).ok_or_else(|| OpError {
                path: source.clone(), kind: OpErrorKind::Msg("A destination filename is required".into()),
            })?;
        // Names are a single component, never a destination path override.
        if Path::new(name).components().count() != 1
            || atlas_core::fs::transfer::validate_filename(&name.to_string_lossy()).is_err() {
            return Err(OpError { path: source.clone(), kind: OpErrorKind::Msg("Invalid destination filename".into()) });
        }
        let destination = req.dest.join(name);
        let (ask, on_conflict) = match &req.policy {
            ConflictPolicy::Ask => (true, Resolution::Rename),
            ConflictPolicy::OverwriteAll => (false, Resolution::Overwrite),
            ConflictPolicy::RenameAll => (false, Resolution::Rename),
            ConflictPolicy::SkipAll => (false, Resolution::Skip),
            ConflictPolicy::PerItem(choices) => {
                let choice = if destination.symlink_metadata().is_ok() {
                    let choice = choices.get(conflict_index).copied().unwrap_or(Resolution::Skip);
                    conflict_index += 1;
                    choice
                } else { Resolution::Rename };
                (false, choice)
            }
        };
        // Explorer's Copy/Paste into the source folder creates another name.
        let self_copy = req.kind == TransferKind::Copy
            && atlas_core::fs::transfer::paths_equal_ci(source, &destination);
        Ok(NativeTransfer { pair: atlas_core::fs::transfer::TransferPair {
            src: source.clone(), dst: destination, on_conflict,
        }, ask: ask && !self_copy })
    }).collect()
}

fn transfer_flags(item: &NativeTransfer, interactive: bool) -> FILEOPERATION_FLAGS {
    let mut flags = FOF_NOCONFIRMMKDIR;
    if !atlas_core::vault::live::is_private(&item.pair.src)
        && !atlas_core::vault::live::is_private(&item.pair.dst) {
        flags |= FOF_ALLOWUNDO | FOFX_ADDUNDORECORD;
    }
    if interactive {
        flags |= FOFX_SHOWELEVATIONPROMPT;
    } else {
        // Fixtures suppress every dialog and stop on a real execution error.
        flags |= FOF_SILENT | FOF_NOERRORUI | FOF_NOCONFIRMATION | FOFX_EARLYFAILURE;
    }
    if !item.ask {
        match item.pair.on_conflict {
            Resolution::Rename => flags |= FOF_RENAMEONCOLLISION,
            Resolution::Overwrite => flags |= FOF_NOCONFIRMATION,
            Resolution::Skip => {}
        }
    }
    flags
}

/// One STA and one IFileOperation per consecutive group of compatible flags.
/// Ordinary Ask requests share a single native conflict/progress workflow.
fn transfer_sta(
    items: Vec<NativeTransfer>, kind: TransferKind, owner: Option<isize>,
    cancel: Arc<AtomicBool>, interactive: bool,
    progress: Option<(u64, flume::Sender<atlas_model::app_state::BgResult>)>,
) -> atlas_model::ops::TransferReport {
    use atlas_model::ops::TransferReport;
    let mut report = TransferReport::default();
    let run = (|| -> windows::core::Result<()> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?; }
        let _apartment = Apartment;
        let mut remaining = items.as_slice();
        while !remaining.is_empty() {
            if cancel.load(Ordering::Relaxed) { report.cancelled = true; break; }
            let flags = transfer_flags(&remaining[0], interactive);
            let count = remaining.iter().take_while(|item| transfer_flags(item, interactive) == flags).count();
            let (group, rest) = remaining.split_at(count);
            remaining = rest;
            let api: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL)? };
            unsafe {
                api.SetOperationFlags(flags)?;
                if let Some(owner) = owner { api.SetOwnerWindow(HWND(owner as *mut _))?; }
            }
            let global_state = Arc::new(Mutex::new(ItemResult::default()));
            let global: IFileOperationProgressSink = Progress {
                source: PathBuf::new(), state: global_state.clone(),
                cancel: cancel.clone(), progress: progress.clone(),
                skip_destination: None,
            }.into();
            let cookie = unsafe { api.Advise(&global)? };
            let mut queued = Vec::new();
            for item in group {
                // Explicit Skip is not a success. A later race is checked again
                // in the pre-item callback (never silently authorize replacement).
                if !item.ask && item.pair.on_conflict == Resolution::Skip
                    && item.pair.dst.symlink_metadata().is_ok() {
                    report.skipped.push(item.pair.src.clone());
                    continue;
                }
                let state = Arc::new(Mutex::new(ItemResult::default()));
                let sink: IFileOperationProgressSink = Progress {
                    source: item.pair.src.clone(), state: state.clone(), cancel: cancel.clone(), progress: None,
                    skip_destination: (!item.ask && item.pair.on_conflict == Resolution::Skip).then(|| item.pair.dst.clone()),
                }.into();
                let queue = (|| -> windows::core::Result<()> { unsafe {
                    let source: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide_path(&item.pair.src).as_ptr()), None)?;
                    let parent = item.pair.dst.parent().ok_or_else(|| windows::core::Error::from_hresult(HRESULT::from_win32(3)))?;
                    let folder: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide_path(parent).as_ptr()), None)?;
                    let name = wide(item.pair.dst.file_name().unwrap_or_default());
                    if kind == TransferKind::Copy { api.CopyItem(&source, &folder, PCWSTR(name.as_ptr()), &sink) }
                    else { api.MoveItem(&source, &folder, PCWSTR(name.as_ptr()), &sink) }
                }})();
                match queue {
                    Ok(()) => queued.push((item, state, sink)),
                    Err(error) => report.errors.push(OpError { path: item.pair.src.clone(), kind: error_kind(&error) }),
                }
            }
            let performed = if queued.is_empty() { Ok(()) } else { unsafe { api.PerformOperations() } };
            let aborted = unsafe { api.GetAnyOperationsAborted() };
            let _ = unsafe { api.Unadvise(cookie) };
            let global_state = global_state.lock().unwrap_or_else(|poison| poison.into_inner());
            let mut unreported = Vec::new();
            for (item, state, _sink) in queued {
                let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
                // Also use the advised sink: descendant notifications can arrive
                // there independently of the sink passed to CopyItem/MoveItem.
                for (path, result) in &global_state.failures {
                    if path.starts_with(&item.pair.src) && !state.failures.contains(&(path.clone(), *result)) {
                        state.failures.push((path.clone(), *result));
                        state.failure.get_or_insert(*result);
                    }
                }
                for path in &global_state.skipped {
                    if path.starts_with(&item.pair.src) && !state.skipped.contains(path) {
                        state.skipped.push(path.clone());
                        state.failure.get_or_insert(COPYENGINE_S_USER_IGNORED);
                    }
                }
                report.skipped.extend(state.skipped.iter().cloned());
                for (path, result) in &state.failures {
                    let kind = error_kind(&windows::core::Error::from_hresult(*result));
                    if kind == OpErrorKind::Cancelled { report.cancelled = true; }
                    else { report.errors.push(OpError { path: path.clone(), kind }); }
                }
                if state.failure.is_none() && state.result.is_some_and(shell_completed) {
                    if let Some(destination) = &state.destination {
                        report.completed.push((item.pair.src.clone(), destination.clone()));
                        continue;
                    }
                }
                if state.failure.is_none() { unreported.push(item.pair.src.clone()); }
            }
            let finish_error = global_state.failure.filter(|result| result.is_err())
                .map(windows::core::Error::from_hresult);
            let batch_error = performed.err().or_else(|| aborted.as_ref().err().cloned()).or(finish_error);
            if let Some(error) = &batch_error {
                if error_kind(error) == OpErrorKind::Cancelled { report.cancelled = true; }
            }
            // Aborted alone can mean Skip, a failure, or cancellation. Never
            // turn it into a successful result or erase completed earlier items.
            if aborted.as_ref().is_ok_and(|value| value.as_bool())
                && (!unreported.is_empty() || (report.skipped.is_empty() && report.errors.is_empty())) { report.cancelled = true; }
            if cancel.load(Ordering::Relaxed) && !unreported.is_empty() { report.cancelled = true; }
            for path in unreported {
                if !report.cancelled {
                    report.errors.push(OpError { path, kind: batch_error.as_ref().map(error_kind)
                        .unwrap_or_else(|| OpErrorKind::Msg("Windows Shell did not confirm completion; partial output may remain".into())) });
                }
            }
            if let Some(error) = batch_error {
                if !report.cancelled {
                    report.errors.push(OpError { path: group[0].pair.src.clone(), kind: error_kind(&error) });
                }
            }
            if report.cancelled { break; }
        }
        Ok(())
    })();
    if let Err(error) = run {
        report.errors.push(OpError { path: items.first().map(|item| item.pair.src.clone()).unwrap_or_default(), kind: error_kind(&error) });
    }
    // Native callbacks do not establish exclusive ownership of merged/replaced
    // trees. No path-based FileOP inverse or fabricated certificate is issued.
    report
}

pub(crate) async fn transfer_native(
    items: Vec<NativeTransfer>,
    kind: TransferKind,
    origin: atlas_model::ops::TransferOrigin,
    id: u64,
    tx: flume::Sender<atlas_model::app_state::BgResult>,
    cancel: Arc<AtomicBool>,
    affected_tabs: Vec<atlas_model::pane::TabId>,
    owner: Option<isize>,
    clipboard_sequence: Option<u32>,
) {
    let progress = Some((id, tx.clone()));
    let mut report = tokio::task::spawn_blocking(move || {
        std::thread::Builder::new().name("fileop-native-transfer".into())
            .spawn(move || transfer_sta(items, kind, owner, cancel, true, progress))
            .map_err(|error| error.to_string())?
            .join().map_err(|_| "Windows Shell transfer worker failed".to_owned())
    }).await.unwrap_or_else(|error| Err(error.to_string())).unwrap_or_else(|error| {
        let mut report = atlas_model::ops::TransferReport::default();
        report.errors.push(OpError { path: PathBuf::new(), kind: OpErrorKind::Msg(error) });
        report
    });
    report.clipboard_sequence = clipboard_sequence;
    let _ = tx.send(atlas_model::app_state::BgResult::TransferComplete { id, kind, origin, report, affected_tabs });
}

/// Queue one Windows Shell operation for the whole selection. A separate
/// PerformOperations call per chunk caused repeated permanent-delete prompts.
/// Per-source sinks retain the successful subset on cancellation/errors.
fn delete_batch_sta(
    paths: &[PathBuf], recycle: bool, owner: Option<isize>, cancel: &Arc<AtomicBool>,
) -> (Vec<PathBuf>, Vec<OpError>) {
    let mut completed = Vec::new();
    let mut errors = Vec::new();
    let run = (|| -> windows::core::Result<()> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?; }
        let _apartment = Apartment;
        let api: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL)? };
        let mut flags = FOF_NOCONFIRMMKDIR | FOFX_EARLYFAILURE | FOFX_SHOWELEVATIONPROMPT;
        if recycle {
            flags |= FOF_ALLOWUNDO | FOFX_ADDUNDORECORD
                | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING;
        }
        unsafe {
            api.SetOperationFlags(flags)?;
            if let Some(hwnd) = owner { api.SetOwnerWindow(HWND(hwnd as *mut _))?; }
        }
        let mut queued = Vec::with_capacity(paths.len());
        for path in paths {
            if cancel.load(Ordering::Relaxed) { break; }
            let state = Arc::new(Mutex::new(ItemResult::default()));
            let sink: IFileOperationProgressSink = Progress {
                source: path.clone(), state: state.clone(), cancel: cancel.clone(),
                progress: None, skip_destination: None,
            }.into();
            let result = (|| -> windows::core::Result<()> {
                let item: IShellItem = unsafe {
                    SHCreateItemFromParsingName(PCWSTR(wide_path(path).as_ptr()), None)?
                };
                unsafe { api.DeleteItem(&item, &sink) }
            })();
            match result {
                Ok(()) => queued.push((path.clone(), state, sink)),
                Err(error) => errors.push(OpError { path: path.clone(), kind: error_kind(&error) }),
            }
        }
        let performed = if queued.is_empty() { Ok(()) } else { unsafe { api.PerformOperations() } };
        let aborted = unsafe { api.GetAnyOperationsAborted() };
        let batch_error = performed.err().or_else(|| aborted.as_ref().err().cloned());
        for (path, state, _sink) in queued {
            let state = state.lock().unwrap_or_else(|poison| poison.into_inner());
            let result = state.result;
            if state.failure.is_none() && result.is_some_and(shell_completed) {
                completed.push(path);
            } else {
                let kind = state.failure.or(result).filter(|r| r.is_err() || shell_skipped(*r))
                    .map(|hr| error_kind(&windows::core::Error::from_hresult(hr)))
                    .or_else(|| batch_error.as_ref().map(error_kind))
                    .unwrap_or_else(|| if cancel.load(Ordering::Relaxed) {
                        OpErrorKind::Cancelled
                    } else {
                        OpErrorKind::Msg("Windows Shell did not confirm deletion".into())
                    });
                errors.push(OpError { path, kind });
            }
        }
        if aborted.as_ref().is_ok_and(|value| value.as_bool())
            && errors.is_empty() && completed.is_empty() && !paths.is_empty() {
            errors.push(OpError { path: paths[0].clone(), kind: OpErrorKind::Cancelled });
        }
        Ok(())
    })();
    if let Err(error) = run {
        // A setup error occurs before PerformOperations: no deletion is asserted.
        let unreported: Vec<_> = paths.iter().filter(|path| !completed.contains(path)
            && !errors.iter().any(|entry| &entry.path == *path)).cloned().collect();
        for path in unreported {
            errors.push(OpError { path, kind: error_kind(&error) });
        }
    }
    (completed, errors)
}

// Opt-in, path-free diagnostic for native folder recycle routing.
// Never changes deletion decisions and never logs filenames.
fn trace_fast_recycle(outcome: &str, elapsed: std::time::Duration) {
    if std::env::var_os("FLDR_FAST_RECYCLE_TRACE").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    use std::io::Write;
    let log = std::env::temp_dir().join("FLDR-Fast-Recycle-Trace.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(log) {
        let _ = writeln!(file, "event={} elapsed_ms={}", outcome, elapsed.as_millis());
    }
}

pub(crate) async fn delete_files(
    paths: Vec<PathBuf>,
    recycle: bool,
    id: u64,
    tx: flume::Sender<atlas_model::app_state::BgResult>,
    cancel: Arc<AtomicBool>,
    affected_tabs: Vec<atlas_model::pane::TabId>,
    owner: Option<isize>,
) {
    use atlas_model::app_state::BgResult;
    let total = paths.len() as u64;
    let worker_tx = tx.clone();
    let result = tokio::task::spawn_blocking(move || {
        // Explicit STA, independent of Tokio's worker apartment state.
        std::thread::Builder::new().name("fileop-native-delete".into())
            .spawn(move || {
                let mut completed = Vec::new();
                let mut errors = Vec::new();
                // Experimental guarded fast recycle: only explicitly enabled local NTFS folders.
                // Every ineligible case still uses the original Shell batching path.
                let mut eligible = Vec::new();
                for (index, path) in paths.iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        errors.extend(paths[index..].iter().cloned().map(|path| OpError {
                            path, kind: OpErrorKind::Cancelled,
                        }));
                        break;
                    }
                    let _ = worker_tx.send(BgResult::OpProgress {
                        id, done: index as u64, total,
                        bytes_done: 0, bytes_total: 0, current_file: path.clone(),
                    });
                    if recycle && atlas_core::fs::paths::wsl_distro_name(path).is_some() {
                        errors.push(OpError {
                            path: path.clone(), kind: OpErrorKind::TrashUnsupported,
                        });
                    } else if recycle && fast_recycle::enabled() {
                        let started = std::time::Instant::now();
                        let outcome = fast_recycle::try_folder(path, &cancel);
                        let status = match &outcome {
                            fast_recycle::Outcome::Moved => "native_success",
                            fast_recycle::Outcome::Fallback => "shell_fallback",
                            fast_recycle::Outcome::Cancelled => "native_cancelled",
                            fast_recycle::Outcome::Error(_) => "native_error",
                        };
                        trace_fast_recycle(status, started.elapsed());
                        match outcome {
                            fast_recycle::Outcome::Moved => completed.push(path.clone()),
                            fast_recycle::Outcome::Fallback => eligible.push(path.clone()),
                            fast_recycle::Outcome::Cancelled => errors.push(OpError {
                                path: path.clone(), kind: OpErrorKind::Cancelled,
                            }),
                            fast_recycle::Outcome::Error(message) => errors.push(OpError {
                                path: path.clone(), kind: OpErrorKind::Msg(message),
                            }),
                        }
                    } else {
                        eligible.push(path.clone());
                    }
                }
                // A pre-cancelled operation must never start deleting the
                // already-queued prefix after the user has asked to stop.
                if cancel.load(Ordering::Relaxed) {
                    errors.extend(eligible.into_iter().map(|path| OpError {
                        path, kind: OpErrorKind::Cancelled,
                    }));
                } else if !eligible.is_empty() {
                    let (done, failed) = delete_batch_sta(&eligible, recycle, owner, &cancel);
                    completed.extend(done);
                    errors.extend(failed);
                }
                (completed, errors)
            }).map_err(|error| error.to_string())?
            .join().map_err(|_| "Windows Shell delete worker panicked".to_owned())
    }).await;
    let (completed, errors) = match result {
        Ok(Ok(report)) => report,
        Ok(Err(message)) => (Vec::new(), vec![OpError {
            path: PathBuf::new(), kind: OpErrorKind::Msg(message),
        }]),
        Err(error) => (Vec::new(), vec![OpError {
            path: PathBuf::new(), kind: OpErrorKind::Msg(error.to_string()),
        }]),
    };
    if !completed.is_empty() { let _ = tx.send(BgResult::PathsDeleted(completed)); }
    let _ = tx.send(BgResult::OpComplete {
        id, result: if errors.is_empty() { Ok(()) } else { Err(errors) }, affected_tabs,
    });
}

pub(crate) async fn rename_file(
    from: PathBuf,
    to: PathBuf,
    id: u64,
    tx: flume::Sender<atlas_model::app_state::BgResult>,
    affected_tabs: Vec<atlas_model::pane::TabId>,
    owner: Option<isize>,
) {
    let operation = Operation::Rename {
        source: from.clone(),
        destination: to,
    };
    let result = tokio::task::spawn_blocking(move || {
        execute(operation, owner, Arc::new(AtomicBool::new(false)), true)
    })
    .await;
    let result = match result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(vec![error]),
        Err(error) => Err(vec![OpError {
            path: from,
            kind: OpErrorKind::Msg(error.to_string()),
        }]),
    };
    let _ = tx.send(atlas_model::app_state::BgResult::OpComplete {
        id,
        result,
        affected_tabs,
    });
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use atlas_model::ops::{ConflictPolicy, TransferOrigin, TransferRequest};

    fn request(sources: Vec<PathBuf>, dest: PathBuf, kind: TransferKind, policy: ConflictPolicy) -> TransferRequest {
        TransferRequest { sources, target_names: None, dest, kind, policy, origin: TransferOrigin::Paste }
    }

    fn native_batch(req: &TransferRequest) -> atlas_model::ops::TransferReport {
        let items = plan_transfer(req).unwrap();
        let kind = req.kind;
        std::thread::spawn(move || transfer_sta(items, kind, None, Arc::new(AtomicBool::new(false)), false, None))
            .join().unwrap()
    }

    #[test]
    fn native_planning_keeps_ask_and_leaves_reparse_and_missing_sources_to_shell() {
        let fixture = Fixture::new();
        let source = fixture.0.join("absent-link");
        let req = request(vec![source.clone()], fixture.0.join("destination"), TransferKind::Copy, ConflictPolicy::Ask);
        let items = plan_transfer(&req).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].ask);
        assert_eq!(items[0].pair.src, source);
        let flags = transfer_flags(&items[0], true);
        for forbidden in [FOF_NOCONFIRMATION, FOF_SILENT, FOF_NOERRORUI, FOFX_EARLYFAILURE, FOF_RENAMEONCOLLISION, FOFX_NOSKIPJUNCTIONS] {
            assert_eq!(flags.0 & forbidden.0, 0);
        }
        assert_ne!(flags.0 & FOFX_SHOWELEVATIONPROMPT.0, 0);
        assert_ne!(flags.0 & FOFX_ADDUNDORECORD.0, 0);
        let mut escape = req;
        escape.target_names = Some(vec!["../escaped".into()]);
        assert!(plan_transfer(&escape).is_err());
    }

    #[tokio::test]
    async fn native_dispatch_completion_keeps_paste_drop_and_menu_context_even_on_cancel() {
        let fixture = Fixture::new();
        let source = fixture.0.join("preserve");
        std::fs::write(&source, b"preserve").unwrap();
        for origin in [TransferOrigin::Paste, TransferOrigin::PasteExternalCut,
            TransferOrigin::DndInternal, TransferOrigin::DndExternal, TransferOrigin::Menu] {
            let mut req = request(vec![source.clone()], fixture.0.join("dest"), TransferKind::Move, ConflictPolicy::Ask);
            req.origin = origin;
            let (tx, rx) = flume::unbounded();
            let affected = vec![atlas_model::layout_tree::TileId(9)];
            transfer_native(plan_transfer(&req).unwrap(), req.kind, origin, 41, tx,
                Arc::new(AtomicBool::new(true)), affected.clone(), None, Some(123)).await;
            match rx.recv().unwrap() {
                atlas_model::app_state::BgResult::TransferComplete { id, kind, origin: actual, report, affected_tabs } => {
                    assert_eq!((id, kind, actual), (41, TransferKind::Move, origin));
                    assert_eq!(affected_tabs, affected);
                    assert_eq!(report.clipboard_sequence, Some(123));
                    assert!(report.cancelled && report.completed.is_empty() && report.reversible.is_empty());
                }
                _ => panic!("Expected native completion"),
            }
            assert!(source.exists());
        }
    }

    #[test]
    fn native_batch_merge_copy_and_move_preserve_unrelated_data_and_disable_inverse() {
        for kind in [TransferKind::Copy, TransferKind::Move] {
            let fixture = Fixture::new();
            let source = fixture.0.join("incoming/tree");
            let dest = fixture.0.join("dest");
            std::fs::create_dir_all(source.join("nested")).unwrap();
            std::fs::create_dir_all(dest.join("tree/nested")).unwrap();
            std::fs::write(source.join("nested/overlap"), b"new").unwrap();
            std::fs::write(source.join("added"), b"added").unwrap();
            std::fs::write(dest.join("tree/nested/overlap"), b"old").unwrap();
            std::fs::write(dest.join("tree/unrelated"), b"unrelated").unwrap();
            let report = native_batch(&request(vec![source.clone()], dest.clone(), kind, ConflictPolicy::OverwriteAll));
            assert!(report.errors.is_empty(), "{report:?}");
            assert!(!report.cancelled, "{report:?}");
            assert_eq!(report.completed, vec![(source.clone(), dest.join("tree"))], "{report:?}");
            assert_eq!(std::fs::read(dest.join("tree/unrelated")).unwrap(), b"unrelated");
            assert_eq!(std::fs::read(dest.join("tree/nested/overlap")).unwrap(), b"new");
            assert_eq!(std::fs::read(dest.join("tree/added")).unwrap(), b"added");
            assert!(report.reversible.is_empty());
            assert_eq!(source.exists(), kind == TransferKind::Copy);
        }
    }

    #[test]
    fn native_batch_reports_partial_queue_failure_and_real_success_without_custom_fallback() {
        let fixture = Fixture::new();
        let source = fixture.0.join("good");
        let missing = fixture.0.join("missing");
        let dest = fixture.0.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(&source, b"good").unwrap();
        let report = native_batch(&request(vec![missing.clone(), source.clone()], dest.clone(), TransferKind::Copy, ConflictPolicy::RenameAll));
        assert_eq!(report.completed, vec![(source.clone(), dest.join("good"))]);
        assert!(report.errors.iter().any(|error| error.path == missing));
        assert!(!report.fully_completed_sources(&[missing, source]));
        assert!(report.reversible.is_empty());
        assert!(!dest.join("missing").exists());
    }

    #[test]
    fn native_descendant_io_failure_is_not_a_completed_folder_or_undo_certificate() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        let source = fixture.0.join("folder");
        let dest = fixture.0.join("dest");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(source.join("open"), b"copyable").unwrap();
        std::fs::write(source.join("locked"), b"retain source").unwrap();
        std::fs::write(dest.join("unrelated"), b"unrelated").unwrap();
        let _locked = std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(source.join("locked")).unwrap();
        let report = native_batch(&request(vec![source.clone()], dest.clone(), TransferKind::Copy, ConflictPolicy::RenameAll));
        assert!(!report.errors.is_empty(), "{report:?}");
        assert!(report.completed.is_empty() && report.reversible.is_empty(), "{report:?}");
        assert!(source.exists());
        assert_eq!(std::fs::read(dest.join("unrelated")).unwrap(), b"unrelated");
    }

    #[test]
    fn native_explicit_per_item_choices_index_conflicts_after_clean_sources() {
        let fixture = Fixture::new();
        let dest = fixture.0.join("dest");
        std::fs::create_dir(&dest).unwrap();
        let clean = fixture.0.join("clean");
        let skip = fixture.0.join("skip");
        let replace = fixture.0.join("replace");
        for source in [&clean, &skip, &replace] { std::fs::write(source, b"incoming").unwrap(); }
        std::fs::write(dest.join("skip"), b"existing").unwrap();
        std::fs::write(dest.join("replace"), b"existing").unwrap();
        let req = request(vec![clean.clone(), skip.clone(), replace], dest.clone(), TransferKind::Copy,
            ConflictPolicy::PerItem(vec![Resolution::Skip, Resolution::Overwrite]));
        let report = native_batch(&req);
        assert!(report.errors.is_empty() && report.completed.len() == 2, "{report:?}");
        assert_eq!(report.skipped, vec![skip]);
        assert_eq!(std::fs::read(dest.join("skip")).unwrap(), b"existing");
        assert_eq!(std::fs::read(dest.join("replace")).unwrap(), b"incoming");
        assert_eq!(std::fs::read(dest.join("clean")).unwrap(), b"incoming");
    }

    #[test]
    fn native_batch_skip_and_keep_both_are_not_overwrite_or_false_success() {
        let fixture = Fixture::new();
        let source = fixture.0.join("same.txt");
        let dest = fixture.0.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(&source, b"incoming").unwrap();
        std::fs::write(dest.join("same.txt"), b"existing").unwrap();
        let report = native_batch(&request(vec![source.clone()], dest.clone(), TransferKind::Move, ConflictPolicy::SkipAll));
        assert!(report.completed.is_empty() && report.errors.is_empty());
        assert_eq!(report.skipped, vec![source.clone()]);
        assert!(!report.fully_completed_sources(&[source.clone()]));
        let report = native_batch(&request(vec![source.clone()], dest.clone(), TransferKind::Copy, ConflictPolicy::RenameAll));
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(report.completed.len(), 1);
        assert_ne!(report.completed[0].1, dest.join("same.txt"));
        assert_eq!(std::fs::read(dest.join("same.txt")).unwrap(), b"existing");
        assert_eq!(std::fs::read(&report.completed[0].1).unwrap(), b"incoming");
        let report = native_batch(&request(vec![source.clone()], fixture.0.clone(), TransferKind::Copy, ConflictPolicy::Ask));
        assert_eq!(report.completed.len(), 1, "{report:?}");
        assert_ne!(report.completed[0].1, source);
    }

    #[test]
    fn native_precancel_batch_and_progress_sink_report_real_work_and_cooperative_cancel() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        let dest = fixture.0.join("dest");
        std::fs::write(&source, b"source").unwrap();
        let items = plan_transfer(&request(vec![source.clone()], dest.clone(), TransferKind::Move, ConflictPolicy::Ask)).unwrap();
        let report = std::thread::spawn(move || transfer_sta(items, TransferKind::Move, None,
            Arc::new(AtomicBool::new(true)), false, None)).join().unwrap();
        assert!(report.cancelled && report.completed.is_empty());
        assert!(source.exists() && !dest.exists());
        let (tx, rx) = flume::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let sink: IFileOperationProgressSink = Progress {
            source, state: Arc::new(Mutex::new(ItemResult::default())), cancel: cancel.clone(),
            progress: Some((7, tx)), skip_destination: None,
        }.into();
        unsafe { sink.UpdateProgress(100, 37).unwrap(); }
        assert!(matches!(rx.recv().unwrap(), atlas_model::app_state::BgResult::OpProgress {
            id: 7, done: 37, total: 100, bytes_done: 0, bytes_total: 0, ..
        }));
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(unsafe { sink.UpdateProgress(100, 38) }.unwrap_err().code(), COPYENGINE_E_USER_CANCELLED);
    }

    #[test]
    fn native_sink_descendant_skip_or_error_prevents_parent_success_and_raced_skip_is_safe() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        std::fs::create_dir(&source).unwrap();
        let child = source.join("child");
        std::fs::write(&child, b"source").unwrap();
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap(); }
        let _apartment = Apartment;
        let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide(source.as_os_str()).as_ptr()), None).unwrap() };
        let child_item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide(child.as_os_str()).as_ptr()), None).unwrap() };
        let state = Arc::new(Mutex::new(ItemResult::default()));
        let sink: IFileOperationProgressSink = Progress {
            source: source.clone(), state: state.clone(), cancel: Arc::new(AtomicBool::new(false)),
            progress: None, skip_destination: Some(source.clone()),
        }.into();
        unsafe {
            sink.PostCopyItem(0, &item, &item, PCWSTR::null(), HRESULT(0), &item).unwrap();
            sink.PostCopyItem(0, &child_item, &item, PCWSTR::null(), COPYENGINE_S_USER_IGNORED, None).unwrap();
            sink.PostCopyItem(0, &child_item, &item, PCWSTR::null(), HRESULT::from_win32(5), None).unwrap();
            assert!(sink.PreCopyItem(0, &item, &item, PCWSTR::null()).is_err());
        }
        let state = state.lock().unwrap();
        assert_eq!(state.result, Some(HRESULT(0)));
        assert_eq!(state.destination, Some(source));
        assert_eq!(state.skipped, vec![child.clone()]);
        assert_eq!(state.failures, vec![(child, HRESULT::from_win32(5))]);
        assert!(state.failure.is_some());
        assert!(!shell_completed(COPYENGINE_S_USER_IGNORED));
        assert!(!shell_completed(COPYENGINE_S_PENDING));
    }

    #[test]
    fn native_copy_and_move_through_junction_destination_use_shell_semantics() {
        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        let junction = fixture.0.join("junction");
        std::fs::create_dir(&target).unwrap();
        let output = std::process::Command::new("cmd.exe").args(["/d", "/c", "mklink", "/J"])
            .arg(&junction).arg(&target).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        for kind in [TransferKind::Copy, TransferKind::Move] {
            let source = fixture.0.join(format!("{kind:?}.txt"));
            std::fs::write(&source, b"native junction destination").unwrap();
            let report = native_batch(&request(vec![source.clone()], junction.clone(), kind, ConflictPolicy::RenameAll));
            assert!(report.errors.is_empty() && report.completed.len() == 1, "{report:?}");
            assert_eq!(std::fs::read(target.join(source.file_name().unwrap())).unwrap(), b"native junction destination");
            assert_eq!(source.exists(), kind == TransferKind::Copy);
            assert!(report.reversible.is_empty());
        }
        std::fs::remove_dir(&junction).unwrap();
    }

    #[test]
    fn native_source_junctions_are_handled_by_shell_without_custom_reparse_rejection() {
        for kind in [TransferKind::Copy, TransferKind::Move] {
            let fixture = Fixture::new();
            let target = fixture.0.join("target");
            let link = fixture.0.join("source-junction");
            let dest = fixture.0.join("dest");
            std::fs::create_dir(&target).unwrap();
            std::fs::create_dir(&dest).unwrap();
            std::fs::write(target.join("sentinel"), b"preserve native link target").unwrap();
            let output = std::process::Command::new("cmd.exe").args(["/d", "/c", "mklink", "/J"])
                .arg(&link).arg(&target).output().unwrap();
            assert!(output.status.success());
            let report = native_batch(&request(vec![link.clone()], dest.clone(), kind, ConflictPolicy::RenameAll));
            assert!(report.errors.is_empty() && report.completed.len() == 1, "{report:?}");
            assert_eq!(std::fs::read(target.join("sentinel")).unwrap(), b"preserve native link target");
            assert!(report.reversible.is_empty());
            // Unlink only fixture reparse entries before recursive fixture cleanup.
            for path in [&link, &dest.join("source-junction")] {
                if path.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink()) {
                    std::fs::remove_dir(path).unwrap();
                }
            }
        }
    }

    #[test]
    fn native_cancellation_during_batch_retains_completed_items_and_refreshable_partial_results() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        let dest = fixture.0.join("dest");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&dest).unwrap();
        let mut paths = Vec::new();
        for index in 0..256 {
            let path = source.join(format!("{index}.bin"));
            std::fs::write(&path, vec![index as u8; 65536]).unwrap();
            paths.push(path);
        }
        let items = plan_transfer(&request(paths.clone(), dest, TransferKind::Copy, ConflictPolicy::RenameAll)).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = flume::unbounded();
        let flag = cancel.clone();
        let monitor = std::thread::spawn(move || {
            rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
            flag.store(true, Ordering::Relaxed);
        });
        let report = std::thread::spawn(move || transfer_sta(items, TransferKind::Copy, None, cancel, false, Some((1, tx))))
            .join().unwrap();
        monitor.join().unwrap();
        assert!(report.cancelled, "{report:?}");
        assert!(report.completed.len() < paths.len());
        assert!(!report.fully_completed_sources(&paths));
        assert!(paths.iter().all(|path| path.exists()));
        for (source, destination) in &report.completed {
            assert_eq!(std::fs::read(source).unwrap(), std::fs::read(destination).unwrap());
        }
        assert!(report.reversible.is_empty());
    }

    #[test]
    #[ignore = "requires Windows symlink privilege or Developer Mode; run explicitly when available"]
    fn native_symbolic_link_copy_and_move_are_not_rejected_by_custom_guards() {
        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        let dest = fixture.0.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(&target, b"unrelated target").unwrap();
        for kind in [TransferKind::Copy, TransferKind::Move] {
            let source = fixture.0.join(format!("{kind:?}-link"));
            std::os::windows::fs::symlink_file(&target, &source).unwrap();
            let report = native_batch(&request(vec![source.clone()], dest.clone(), kind, ConflictPolicy::RenameAll));
            assert!(report.errors.is_empty() && report.completed.len() == 1, "{report:?}");
            assert_eq!(std::fs::read(&target).unwrap(), b"unrelated target");
            assert_eq!(source.symlink_metadata().is_ok(), kind == TransferKind::Copy);
            assert!(report.reversible.is_empty());
        }
    }

    #[test]
    #[ignore = "requires FILEOP_TEST_SECOND_VOLUME_TEMP pointing to a disposable directory on another volume"]
    fn native_cross_volume_file_and_folder_copy_and_move_use_shell() {
        let fixture = Fixture::new();
        let other_root = PathBuf::from(std::env::var_os("FILEOP_TEST_SECOND_VOLUME_TEMP").unwrap());
        assert!(other_root.is_absolute());
        let resolved_root = other_root.canonicalize().unwrap();
        let other = other_root.join(fixture.0.file_name().unwrap());
        assert!(!atlas_core::fs::paths::same_volume(&fixture.0, &other));
        assert!(other.starts_with(&other_root) && other.file_name() == fixture.0.file_name());
        std::fs::create_dir(&other).unwrap();
        for kind in [TransferKind::Copy, TransferKind::Move] {
        for directory in [false, true] {
            let source = fixture.0.join(format!("{kind:?}-{}", if directory { "folder" } else { "file" }));
            if directory { std::fs::create_dir(&source).unwrap(); std::fs::write(source.join("child"), b"native cross volume").unwrap(); }
            else { std::fs::write(&source, b"native cross volume").unwrap(); }
            let report = native_batch(&request(vec![source.clone()], other.clone(), kind, ConflictPolicy::RenameAll));
            assert!(report.errors.is_empty() && report.completed.len() == 1, "{report:?}");
            assert_eq!(source.exists(), kind == TransferKind::Copy);
            let copied = &report.completed[0].1;
            assert!(copied.starts_with(&other));
            assert_eq!(std::fs::read(if directory { copied.join("child") } else { copied.clone() }).unwrap(), b"native cross volume");
        }
        }
        assert!(other.canonicalize().unwrap().starts_with(&resolved_root));
        std::fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn shortcut_collision_preserves_existing_link_and_resolves_target() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source with spaces.txt");
        std::fs::write(&source, b"shortcut target").unwrap();
        let first = create_shortcut(source.clone(), fixture.0.clone()).unwrap();
        let bytes = std::fs::read(&first).unwrap();
        let second = create_shortcut(source.clone(), fixture.0.clone()).unwrap();
        assert_ne!(first, second);
        assert!(second.file_name().unwrap().to_string_lossy().ends_with(" (2).lnk"));
        assert_eq!(std::fs::read(&first).unwrap(), bytes);
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            let _apartment = Apartment;
            for path in [first, second] {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_ALL).unwrap();
                let persistence: windows::Win32::System::Com::IPersistFile = link.cast().unwrap();
                persistence.Load(PCWSTR(wide(path.as_os_str()).as_ptr()), windows::Win32::System::Com::STGM_READ).unwrap();
                let mut buffer = [0u16; 32768];
                link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32).unwrap();
                let length = buffer.iter().position(|&unit| unit == 0).unwrap();
                let resolved = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]));
                assert_eq!(resolved, source);
            }
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "atlas-ifileoperation-{}-{stamp}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn run(&self, operation: Operation) -> Option<PathBuf> {
            assert!(
                operation.source().starts_with(&self.0),
                "Fixture source escaped temporary tree"
            );
            execute(operation, None, Arc::new(AtomicBool::new(false)), false).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn native_copy_move_rename_delete_and_collision_use_real_windows_shell() {
        let fixture = Fixture::new();
        let root = &fixture.0;
        let source = root.join("source.txt");
        std::fs::write(&source, b"native fixture").unwrap();
        let copied = root.join("copy.txt");
        assert_eq!(
            fixture.run(Operation::Copy {
                source: source.clone(),
                destination: copied.clone(),
                conflict: Resolution::Rename
            }),
            Some(copied.clone())
        );
        assert_eq!(std::fs::read(&copied).unwrap(), b"native fixture");
        let collision = fixture
            .run(Operation::Copy {
                source: source.clone(),
                destination: copied.clone(),
                conflict: Resolution::Rename,
            })
            .unwrap();
        assert_ne!(collision, copied);
        assert!(collision.starts_with(root));
        assert_eq!(std::fs::read(&collision).unwrap(), b"native fixture");
        let moved = root.join("moved.txt");
        assert_eq!(
            fixture.run(Operation::Move {
                source: copied.clone(),
                destination: moved.clone(),
                conflict: Resolution::Rename
            }),
            Some(moved.clone())
        );
        assert!(!copied.exists());
        assert!(moved.exists());
        let renamed = root.join("renamed.txt");
        assert_eq!(
            fixture.run(Operation::Rename {
                source: moved.clone(),
                destination: renamed.clone()
            }),
            Some(renamed.clone())
        );
        assert!(!moved.exists());
        assert!(renamed.exists());
        fixture.run(Operation::Delete {
            source: renamed.clone(),
            recycle: false,
        });
        assert!(!renamed.exists());
        assert!(source.exists());
    }
    #[test]
    fn native_precancel_preserves_source_and_reports_cancellation() {
        let fixture = Fixture::new();
        let path = fixture.0.join("preserve.txt");
        std::fs::write(&path, b"preserve").unwrap();
        let error = execute(
            Operation::Delete {
                source: path.clone(),
                recycle: false,
            },
            None,
            Arc::new(AtomicBool::new(true)),
            false,
        )
        .unwrap_err();
        assert_eq!(error.kind, OpErrorKind::Cancelled);
        assert!(path.exists());
    }

    #[test]
    fn native_shell_owns_sta_when_calling_worker_is_already_mta() {
        unsafe {
            CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED)
                .ok()
                .unwrap();
        }
        let _apartment = Apartment;
        let fixture = Fixture::new();
        let source = fixture.0.join("mta-source.txt");
        let destination = fixture.0.join("sta-copy.txt");
        std::fs::write(&source, b"apartment-safe fixture").unwrap();
        assert_eq!(
            fixture.run(Operation::Copy {
                source,
                destination: destination.clone(),
                conflict: Resolution::Rename
            }),
            Some(destination)
        );
    }
    #[test]
    fn native_skip_collision_preserves_both_files() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source.txt");
        let destination = fixture.0.join("target.txt");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&destination, b"target").unwrap();
        let error = execute(
            Operation::Copy {
                source: source.clone(),
                destination: destination.clone(),
                conflict: Resolution::Skip,
            },
            None,
            Arc::new(AtomicBool::new(false)),
            false,
        )
        .unwrap_err();
        assert_eq!(error.kind, OpErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&source).unwrap(), b"source");
        assert_eq!(std::fs::read(&destination).unwrap(), b"target");
    }

    #[test]
    fn native_directory_copy_recurses_and_overwrite_obeys_resolved_policy() {
        let fixture = Fixture::new();
        let source = fixture.0.join("folder");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("nested/Unicode-ñ.txt"), b"directory fixture").unwrap();
        let destination = fixture.0.join("folder-copy");
        assert_eq!(
            fixture.run(Operation::Copy {
                source: source.clone(),
                destination: destination.clone(),
                conflict: Resolution::Rename
            }),
            Some(destination.clone())
        );
        assert_eq!(
            std::fs::read(destination.join("nested/Unicode-ñ.txt")).unwrap(),
            b"directory fixture"
        );
        let original = fixture.0.join("original.txt");
        let replace = fixture.0.join("replace.txt");
        std::fs::write(&original, b"new value").unwrap();
        std::fs::write(&replace, b"old value").unwrap();
        assert_eq!(
            fixture.run(Operation::Copy {
                source: original,
                destination: replace.clone(),
                conflict: Resolution::Overwrite
            }),
            Some(replace.clone())
        );
        assert_eq!(std::fs::read(replace).unwrap(), b"new value");
        fixture.run(Operation::Delete {
            source: destination.clone(),
            recycle: false,
        });
        assert!(!destination.exists());
        assert!(source.exists());
    }

    #[test]
    fn native_trash_returns_fixture_payload_and_restores_only_that_item() {
        let fixture = Fixture::new();
        let source = fixture.0.join("trash-fixture.txt");
        std::fs::write(&source, b"fixture recycle and restore").unwrap();
        let recycled = fixture
            .run(Operation::Delete {
                source: source.clone(),
                recycle: true,
            })
            .expect("Shell did not report recycled payload");
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(&recycled).unwrap(),
            b"fixture recycle and restore"
        );
        let name = recycled.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("$R"),
            "Unexpected recycle payload format: {recycled:?}"
        );
        let metadata = recycled.with_file_name(format!("$I{}", &name[2..]));
        assert!(metadata.exists());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, rx) = flume::unbounded();
        rt.block_on(atlas_core::fs::recycle_bin::restore_items(
            vec![(recycled.clone(), metadata.clone(), source.clone())],
            1,
            tx,
            vec![],
        ));
        assert!(rx.try_iter().any(|event| matches!(
            event,
            atlas_model::app_state::BgResult::OpComplete { result: Ok(()), .. }
        )));
        assert_eq!(
            std::fs::read(source).unwrap(),
            b"fixture recycle and restore"
        );
        assert!(!recycled.exists());
        assert!(!metadata.exists());
    }
}
