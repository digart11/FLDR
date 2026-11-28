//! Explicit, cancellable logical-size scan. Never opened from a render/selection change.
use atlas_model::folder_size::SizeReport;
use std::fs::{self, Metadata, ReadDir};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn skip(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Same conservative reparse/offline/recall policy as cleanup::scan.
        metadata.file_attributes() & (0x400 | 0x1000 | 0x40000 | 0x400000) != 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_symlink()
    }
}

#[derive(Clone, Copy)]
struct EntryInfo { size: u64, file: bool, dir: bool, skip: bool }
impl EntryInfo {
    fn from_metadata(meta: &Metadata) -> Self {
        Self { size: meta.len(), file: meta.is_file(), dir: meta.is_dir(), skip: skip(meta) }
    }
    #[cfg(windows)]
    fn native(attributes: u32, size: u64) -> Self {
        Self { size, file: attributes & 0x10 == 0, dir: attributes & 0x10 != 0,
            skip: attributes & (0x400 | 0x1000 | 0x40000 | 0x400000) != 0 }
    }
}

enum Entries {
    Portable(ReadDir),
    #[cfg(windows)]
    Native { cursor: atlas_win_space::directory::StatisticsCursor, path: PathBuf, started: bool },
}
#[cfg(windows)]
fn unsupported(error: &std::io::Error) -> bool {
    // Unsupported information class/provider only. Access, sharing, network and
    // malformed/partial enumeration failures remain errors, never a second walk.
    matches!(error.raw_os_error(), Some(1 | 50 | 87 | 124))
}
impl Entries {
    fn portable(path: &Path) -> std::io::Result<Self> {
        #[cfg(windows)]
        if skip(&fs::symlink_metadata(path)?) {
            return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Directory became a reparse/offline entry"));
        }
        fs::read_dir(path).map(Self::Portable)
    }
    fn open(path: &Path) -> std::io::Result<Self> {
        #[cfg(windows)]
        match atlas_win_space::directory::StatisticsCursor::open(path) {
            Ok(cursor) => return Ok(Self::Native { cursor, path: path.to_path_buf(), started: false }),
            Err(error) if unsupported(&error) => {},
            Err(error) => return Err(error),
        }
        Self::portable(path)
    }

    fn next(&mut self, #[allow(unused_variables)] scratch: &mut Option<Vec<u64>>)
        -> Option<std::io::Result<(PathBuf, Option<EntryInfo>)>> {
        match self {
            Self::Portable(entries) => entries.next().map(|entry| entry.map(|entry| (entry.path(), None))),
            #[cfg(windows)]
            Self::Native { cursor, path, started } => {
                match cursor.next_entry(scratch.get_or_insert_with(|| vec![0; 64 * 1024 / 8])) {
                    Some(Ok(entry)) => {
                        *started = true;
                        Some(Ok((path.join(entry.name), Some(EntryInfo::native(entry.attributes, entry.size)))))
                    }
                    Some(Err(error)) if !*started && unsupported(&error) => {
                        match Self::portable(&*path) {
                            Ok(entries) => { *self = entries; self.next(scratch) }
                            Err(error) => Some(Err(error)),
                        }
                    }
                    Some(Err(error)) => Some(Err(error)),
                    None => None,
                }
            }
        }
    }
}

fn error(report: &mut SizeReport, path: &Path, why: &std::io::Error) {
    report.errors += 1;
    if report.first_error.is_none() {
        report.first_error = Some(format!("{}: {why}", path.display()));
    }
}

/// Compatibility selected-total API, backed by the same traversal as directory statistics.
pub fn scan(paths: Vec<PathBuf>, cancel: &AtomicBool, excluded: impl Fn(&Path) -> bool,
    publish: impl FnMut(SizeReport)) -> SizeReport {
    scan_with_stats(paths, cancel, excluded, publish, |_| {})
}

/// One depth-bounded post-order traversal. Each finished directory publishes its
/// independent recursive totals, excluding itself from its subfolder count.
/// Selected ancestors cover descendants; a descendant still gets its own result.
pub fn scan_with_stats(
    paths: Vec<PathBuf>, cancel: &AtomicBool, excluded: impl Fn(&Path) -> bool,
    mut publish: impl FnMut(SizeReport),
    mut directory_ready: impl FnMut(atlas_model::folder_stats::DirectoryStats),
) -> SizeReport {
    use atlas_model::folder_stats::DirectoryStats;
    struct Frame { path: PathBuf, entries: Entries, before: SizeReport }
    fn delta(report: &SizeReport, before: &SizeReport, cancelled: bool) -> SizeReport {
        SizeReport { bytes: report.bytes.saturating_sub(before.bytes), files: report.files.saturating_sub(before.files),
            folders: report.folders.saturating_sub(before.folders), skipped: report.skipped.saturating_sub(before.skipped),
            errors: report.errors.saturating_sub(before.errors), first_error: if report.errors > before.errors && before.errors == 0 { report.first_error.clone() } else { None },
            cancelled, done: true }
    }
    let mut report = SizeReport::default();
    let mut roots = Vec::new();
    // Canonicalize only on this worker and only after refusing the link itself.
    for original in paths {
        if cancel.load(Ordering::Relaxed) { break; }
        let before = report.clone();
        if excluded(&original) { report.skipped += 1; }
        else {
            match fs::symlink_metadata(&original) {
                Ok(meta) if skip(&meta) => report.skipped += 1,
                Ok(meta) => match fs::canonicalize(&original) {
                    Ok(canonical) => { roots.push((canonical, original, meta.is_dir())); continue; }
                    Err(why) => error(&mut report, &original, &why),
                },
                Err(why) => error(&mut report, &original, &why),
            }
        }
        directory_ready(DirectoryStats::new(original, delta(&report, &before, false)));
    }
    let mut requested_aliases: std::collections::HashMap<PathBuf, Vec<PathBuf>> = std::collections::HashMap::new();
    for (canonical, original, is_dir) in &roots {
        if *is_dir { requested_aliases.entry(canonical.clone()).or_default().push(original.clone()); }
    }
    roots.sort_by_key(|(path, _, _)| path.components().count());
    let mut unique: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    let mut covered = std::collections::HashSet::new();
    for root in roots {
        if !root.0.ancestors().any(|path| covered.contains(path)) {
            covered.insert(root.0.clone()); unique.push(root);
        }
    }
    let mut last = Instant::now();
    let mut directory_scratch = None;
    for (root, original, _is_directory) in unique {
        let mut pending = Some((root.clone(), None));
        let mut stack: Vec<Frame> = Vec::new();
        let public_path = |path: &Path| original.join(path.strip_prefix(&root).unwrap_or(Path::new("")));
        let mut emit = |path: &Path, result: SizeReport| {
            let primary = public_path(path);
            directory_ready(DirectoryStats::new(primary.clone(), result.clone()));
            if let Some(aliases) = requested_aliases.get(path) {
                for alias in aliases {
                    if atlas_model::folder_stats::key(alias) != atlas_model::folder_stats::key(&primary) {
                        directory_ready(DirectoryStats::new(alias.clone(), result.clone()));
                    }
                }
            }
        };
        loop {
            if cancel.load(Ordering::Relaxed) { break; }
            let (path, cached) = if let Some(entry) = pending.take() { entry }
            else if let Some(frame) = stack.last_mut() {
                match frame.entries.next(&mut directory_scratch) {
                    Some(Ok(entry)) => entry,
                    Some(Err(why)) => { error(&mut report, &frame.path, &why); continue; }
                    None => {
                        let frame = stack.pop().unwrap();
                        emit(&frame.path, delta(&report, &frame.before, false));
                        continue;
                    }
                }
            } else { break; };
            let before = report.clone();
            let mut terminal_directory = false;
            if excluded(&path) { report.skipped += 1; terminal_directory = true; }
            else {
                match cached.map(Ok).unwrap_or_else(|| fs::symlink_metadata(&path).map(|meta| EntryInfo::from_metadata(&meta))) {
                    Ok(meta) if meta.skip => { report.skipped += 1; terminal_directory = true; }
                    Ok(meta) if meta.file => {
                        report.bytes = report.bytes.saturating_add(meta.size);
                        report.files = report.files.saturating_add(1);
                    }
                    Ok(meta) if meta.dir => {
                        if path != root { report.folders = report.folders.saturating_add(1); }
                        let own_start = report.clone();
                        if stack.len() >= 256 { report.skipped += 1; terminal_directory = true; }
                        else {
                            match Entries::open(&path) {
                                Ok(entries) => stack.push(Frame { path: path.clone(), entries, before: own_start }),
                                Err(why) => { error(&mut report, &path, &why); terminal_directory = true; }
                            }
                        }
                    }
                    Ok(_) => { report.skipped += 1; }
                    Err(why) => { error(&mut report, &path, &why); terminal_directory = true; }
                }
            }
            if terminal_directory {
                let mut result = delta(&report, &before, false);
                result.folders = 0; // This directory is not its own descendant.
                emit(&path, result);
            }
            if last.elapsed() >= Duration::from_millis(150) { publish(report.clone()); last = Instant::now(); }
        }
        if cancel.load(Ordering::Relaxed) {
            for frame in stack.into_iter().rev() {
                emit(&frame.path, delta(&report, &frame.before, true));
            }
            break;
        }
    }
    report.cancelled = cancel.load(Ordering::Relaxed);
    report.done = true;
    publish(report.clone());
    report
}

/// Bounded automatic batch wrapper around the existing traversal, not a second
/// scanner. Callers supply visible sibling roots; cancellation is shared and the
/// only terminal progress message is emitted after every worker has returned.
pub fn scan_automatic(
    mut paths: Vec<PathBuf>, cancel: &AtomicBool, concurrency: usize,
    publish: impl Fn(SizeReport) + Sync,
    directory_ready: impl Fn(atlas_model::folder_stats::DirectoryStats) + Sync,
) -> SizeReport {
    paths.sort_by_key(|p| p.components().count());
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        let key = atlas_model::folder_stats::key(&path);
        if !unique.iter().any(|p| key.starts_with(atlas_model::folder_stats::key(p))) { unique.push(path); }
    }
    unique.truncate(concurrency.clamp(1, 2));
    let reports = std::sync::Mutex::new(vec![SizeReport::default(); unique.len()]);
    fn aggregate(reports: &[SizeReport], done: bool, cancelled: bool) -> SizeReport {
        let mut total = SizeReport { done, cancelled, ..Default::default() };
        for r in reports {
            total.bytes = total.bytes.saturating_add(r.bytes);
            total.files = total.files.saturating_add(r.files);
            total.folders = total.folders.saturating_add(r.folders);
            total.errors = total.errors.saturating_add(r.errors);
            total.skipped = total.skipped.saturating_add(r.skipped);
            total.cancelled |= r.cancelled;
            if total.first_error.is_none() { total.first_error = r.first_error.clone(); }
        }
        total
    }
    let run = |index, path: PathBuf| {
        #[cfg(windows)]
        let _priority = atlas_win_space::storage::BackgroundStatisticsPriority::enter();
        let paced = std::cell::Cell::new(Instant::now());
        scan_with_stats(vec![path], cancel, |path| {
            if paced.get().elapsed() >= Duration::from_millis(8) {
                std::thread::sleep(Duration::from_millis(2));
                paced.set(Instant::now());
            }
            path.parent().is_none() || super::scan_policy::is_statistics_excluded(path)
        }, |report| {
            let mut reports = reports.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            reports[index] = report;
            // Hold this lock through publication to preserve progress ordering.
            publish(aggregate(&reports, false, cancel.load(Ordering::Relaxed)));
        }, &directory_ready);
    };
    if unique.len() < 2 {
        for (index, path) in unique.into_iter().enumerate() { run(index, path); }
    } else {
        std::thread::scope(|scope| {
            for (index, path) in unique.into_iter().enumerate() {
                let run = &run;
                scope.spawn(move || run(index, path));
            }
        });
    }
    let total = aggregate(&reports.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner), true, cancel.load(Ordering::Relaxed));
    publish(total.clone());
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn cached_attributes_keep_all_existing_reparse_and_cloud_exclusions() {
        for flags in [0x400, 0x1000, 0x40000, 0x400000, 0x410, 0x1010] {
            assert!(EntryInfo::native(flags, 123).skip);
        }
        assert!(!EntryInfo::native(0x20, 123).skip);
        assert!(EntryInfo::native(0x10, 0).dir);
        assert!(EntryInfo::native(0x20, 123).file);
        assert_eq!(EntryInfo::native(0x20, 123).size, 123);
        for code in [1, 50, 87, 124] { assert!(unsupported(&std::io::Error::from_raw_os_error(code))); }
        for code in [2, 3, 5, 32, 53, 64] { assert!(!unsupported(&std::io::Error::from_raw_os_error(code))); }
        assert!(!unsupported(&std::io::Error::new(std::io::ErrorKind::InvalidData, "partial page")));
    }

    #[cfg(windows)]
    #[test]
    #[allow(unsafe_code)] // Disposable attribute fixture only; scanner consumers stay safe.
    fn native_offline_record_remains_partial_without_reading_file_content() {
        use std::os::windows::ffi::OsStrExt;
        use windows::{core::PCWSTR, Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_OFFLINE}};
        let fixture = Fixture::new(); let file = fixture.0.join("offline");
        fs::write(&file, [0; 11]).unwrap();
        let name: Vec<_> = file.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe { SetFileAttributesW(PCWSTR(name.as_ptr()), FILE_ATTRIBUTE_OFFLINE).unwrap(); }
        let mut entries = Vec::new();
        let report = scan_with_stats(vec![fixture.0.clone()], &AtomicBool::new(false), |_| false, |_| {}, |entry| entries.push(entry));
        unsafe { SetFileAttributesW(PCWSTR(name.as_ptr()), FILE_ATTRIBUTE_NORMAL).unwrap(); }
        assert_eq!((report.files, report.bytes, report.skipped), (0, 0, 1));
        assert!(entries.iter().any(|entry| entry.path == fixture.0 && entry.state == atlas_model::folder_stats::State::Partial));
    }

    #[test]
    fn original_depth_limit_still_marks_the_root_incomplete() {
        let fixture = Fixture::new();
        let mut path = fixture.0.canonicalize().unwrap();
        for _ in 0..258 { path.push("d"); fs::create_dir(&path).unwrap(); }
        fs::write(path.join("beyond-limit"), [0; 23]).unwrap();
        let report = scan(vec![fixture.0.clone()], &AtomicBool::new(false), |_| false, |_| {});
        assert_eq!((report.files, report.bytes, report.folders, report.skipped), (0, 0, 256, 1));
    }

    #[cfg(windows)]
    #[test]
    fn paged_native_and_portable_entries_agree_and_cancel_inside_a_page() {
        let fixture = Fixture::new();
        for index in 0..2048 { fs::write(fixture.0.join(format!("{index:04}")), [0; 3]).unwrap(); }
        let mut native = Entries::open(&fixture.0).unwrap();
        assert!(matches!(native, Entries::Native { .. }));
        let mut portable = Entries::Portable(fs::read_dir(&fixture.0).unwrap());
        let mut scratch = None;
        let mut left = Vec::new();
        while let Some(entry) = native.next(&mut scratch) {
            let (path, meta) = entry.unwrap();
            let meta = meta.expect("native record lost cached metadata");
            left.push((path, meta.size, meta.file, meta.dir, meta.skip));
        }
        let mut right = Vec::new();
        while let Some(entry) = portable.next(&mut scratch) {
            let (path, cached) = entry.unwrap(); assert!(cached.is_none());
            let meta = EntryInfo::from_metadata(&fs::symlink_metadata(&path).unwrap());
            right.push((path, meta.size, meta.file, meta.dir, meta.skip));
        }
        left.sort(); right.sort(); assert_eq!(left, right);
        let cancel = AtomicBool::new(false);
        let visited = std::cell::Cell::new(0);
        let mut descendants = Vec::new();
        let report = scan_with_stats(vec![fixture.0.clone()], &cancel, |_| {
            visited.set(visited.get() + 1);
            if visited.get() == 17 { cancel.store(true, Ordering::Relaxed); }
            false
        }, |_| {}, |entry| descendants.push(entry));
        assert!(report.cancelled && report.files > 0 && report.files < 2048);
        assert!(descendants.iter().any(|entry| entry.path == fixture.0 && entry.state == atlas_model::folder_stats::State::Cancelled));
    }

    #[cfg(windows)]
    #[test]
    fn directory_removed_after_enumeration_is_an_error_not_a_completed_subtree() {
        let fixture = Fixture::new();
        let child = fixture.0.join("child"); fs::create_dir(&child).unwrap();
        let mut entries = Vec::new();
        let report = scan_with_stats(vec![fixture.0.clone()], &AtomicBool::new(false), |path| {
            if path.ends_with("child") { fs::remove_dir(path).unwrap(); }
            false
        }, |_| {}, |entry| entries.push(entry));
        assert_eq!(report.errors, 1);
        assert!(entries.iter().any(|entry| entry.path == child && entry.state == atlas_model::folder_stats::State::Failed));
        assert!(entries.iter().any(|entry| entry.path == fixture.0 && entry.state == atlas_model::folder_stats::State::Partial));
    }

    #[cfg(windows)]
    #[test]
    fn replaced_directory_junction_is_not_entered_even_on_unsupported_fallback() {
        let fixture = Fixture::new();
        let target = Fixture::new();
        fs::write(target.0.join("sentinel"), [0; 91]).unwrap();
        let child = fixture.0.join("child"); fs::create_dir(&child).unwrap();
        let report = scan(vec![fixture.0.clone()], &AtomicBool::new(false), |path| {
            if path.ends_with("child") {
                fs::remove_dir(path).unwrap();
                assert!(std::process::Command::new("cmd.exe").args(["/d", "/c", "mklink", "/J"])
                    .arg(path).arg(&target.0).output().unwrap().status.success());
            }
            false
        }, |_| {});
        fs::remove_dir(&child).unwrap();
        assert_eq!((report.bytes, report.files), (0, 0));
        assert!(report.errors > 0 || report.skipped > 0);
        assert_eq!(fs::read(target.0.join("sentinel")).unwrap().len(), 91);
    }

    #[cfg(windows)]
    #[test]
    fn inaccessible_child_retains_partial_root_and_error_accounting() {
        let fixture = Fixture::new();
        let child = fixture.0.join("denied"); fs::create_dir(&child).unwrap();
        fs::write(child.join("hidden"), [0; 9]).unwrap();
        fs::write(fixture.0.join("visible"), [0; 4]).unwrap();
        let who = std::process::Command::new("whoami.exe").args(["/user", "/fo", "csv", "/nh"]).output().unwrap();
        assert!(who.status.success());
        let text = String::from_utf8(who.stdout).unwrap();
        let sid = text.trim().split(',').nth(1).unwrap().trim_matches('"');
        assert!(sid.starts_with("S-1-"));
        let deny = std::process::Command::new("icacls.exe").arg(&child).arg("/deny").arg(format!("*{sid}:(RD)")).output().unwrap();
        assert!(deny.status.success());
        let report = scan(vec![fixture.0.clone()], &AtomicBool::new(false), |_| false, |_| {});
        let restore = std::process::Command::new("icacls.exe").arg(&child).arg("/remove:d").arg(format!("*{sid}")).output().unwrap();
        assert!(restore.status.success());
        assert_eq!((report.bytes, report.files, report.errors), (4, 1, 1));
        assert!(report.first_error.is_some());
    }

    #[cfg(windows)]
    #[test]
    fn file_size_changes_use_enumeration_snapshot_and_next_scan_observes_new_size() {
        let fixture = Fixture::new(); let file = fixture.0.join("file");
        fs::write(&file, [0; 3]).unwrap();
        let first = scan(vec![fixture.0.clone()], &AtomicBool::new(false), |path| {
            if path.ends_with("file") { fs::write(path, [0; 19]).unwrap(); }
            false
        }, |_| {});
        assert_eq!(first.bytes, 3); // Captured in the parent enumeration, before the callback.
        let second = scan(vec![fixture.0.clone()], &AtomicBool::new(false), |_| false, |_| {});
        assert_eq!(second.bytes, 19);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires existing Windows symlink privilege or Developer Mode"]
    fn file_directory_and_broken_symbolic_links_remain_skipped() {
        let fixture = Fixture::new(); let target = Fixture::new();
        let file = target.0.join("file"); fs::write(&file, [0; 9]).unwrap();
        std::os::windows::fs::symlink_file(&file, fixture.0.join("file-link")).unwrap();
        std::os::windows::fs::symlink_dir(&target.0, fixture.0.join("dir-link")).unwrap();
        std::os::windows::fs::symlink_file(target.0.join("absent"), fixture.0.join("broken")).unwrap();
        let report = scan(vec![fixture.0.clone()], &AtomicBool::new(false), |_| false, |_| {});
        assert_eq!((report.files, report.bytes, report.skipped), (0, 0, 3));
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "fileop-size-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn recursive_statistics_are_independent_and_overlaps_walk_once() {
        let fixture = Fixture::new(); let root = &fixture.0;
        fs::create_dir(root.join("child")).unwrap(); fs::create_dir(root.join("empty")).unwrap();
        fs::write(root.join("a"), [0; 5]).unwrap(); fs::write(root.join("child/b"), [0; 7]).unwrap();
        let mut entries = Vec::new();
        let result = scan_with_stats(vec![root.clone(), root.join("child")], &AtomicBool::new(false), |_| false, |_| {}, |stats| entries.push(stats));
        let entry = |path: &Path| entries.iter().find(|e| atlas_model::folder_stats::key(&e.path) == atlas_model::folder_stats::key(path)).unwrap();
        assert_eq!((result.bytes, result.files, result.folders), (12, 2, 2));
        assert_eq!((entry(root).report.bytes, entry(root).report.files, entry(root).report.folders), (12, 2, 2));
        assert_eq!((entry(&root.join("child")).report.bytes, entry(&root.join("child")).report.files, entry(&root.join("child")).report.folders), (7, 1, 0));
        assert_eq!(entry(&root.join("empty")).report.bytes, 0);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.state == atlas_model::folder_stats::State::Completed));
    }
    #[test]
    fn sibling_roots_have_separate_results_and_cancelled_parent_is_not_exact() {
        let fixture = Fixture::new(); let root = &fixture.0;
        fs::create_dir(root.join("a")).unwrap(); fs::create_dir(root.join("b")).unwrap();
        fs::write(root.join("a/file"), [0; 3]).unwrap(); fs::write(root.join("b/file"), [0; 8]).unwrap();
        let mut results = Vec::new();
        scan_with_stats(vec![root.join("a"), root.join("b")], &AtomicBool::new(false), |_| false, |_| {}, |e| results.push(e));
        assert_eq!(results.iter().map(|e| e.report.bytes).collect::<Vec<_>>(), [3, 8]);
        let cancel = AtomicBool::new(false); let mut results = Vec::new();
        let report = scan_with_stats(vec![root.clone()], &cancel, |_| false, |_| {}, |e| { cancel.store(true, Ordering::Relaxed); results.push(e); });
        assert!(report.cancelled);
        assert!(results.iter().any(|e| e.path == *root && e.state == atlas_model::folder_stats::State::Cancelled));
    }
    #[test]
    fn overlaps_duplicates_and_empty_folders_do_not_inflate_totals() {
        let fixture = Fixture::new();
        let root = &fixture.0;
        fs::create_dir(root.join("child")).unwrap();
        fs::create_dir(root.join("empty")).unwrap();
        fs::write(root.join("a"), [0; 5]).unwrap();
        fs::write(root.join("child/b"), [0; 7]).unwrap();
        let result = scan(
            vec![
                root.join("child/b"),
                root.clone(),
                root.join("child"),
                root.clone(),
            ],
            &AtomicBool::new(false),
            |_| false,
            |_| {},
        );
        assert_eq!(
            (result.bytes, result.files, result.errors, result.skipped),
            (12, 2, 0, 0)
        );
    }
    #[test]
    fn cancellation_and_missing_roots_are_explicit_partial_results() {
        let fixture = Fixture::new();
        let result = scan(
            vec![fixture.0.clone()],
            &AtomicBool::new(true),
            |_| false,
            |_| {},
        );
        assert!(result.cancelled && result.done);
        assert_eq!(result.bytes, 0);
        let result = scan(
            vec![fixture.0.join("missing")],
            &AtomicBool::new(false),
            |_| false,
            |_| {},
        );
        assert_eq!(result.errors, 1);
        assert!(result.first_error.is_some());
    }
    #[test]
    fn protected_subtrees_are_skipped_without_entering_them() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("private")).unwrap();
        fs::write(fixture.0.join("private/hidden"), [0; 10]).unwrap();
        fs::write(fixture.0.join("visible"), [0; 3]).unwrap();
        let report = scan(
            vec![fixture.0.clone()],
            &AtomicBool::new(false),
            |path| path.ends_with("private"),
            |_| {},
        );
        assert_eq!((report.bytes, report.files, report.skipped), (3, 1, 1));
    }
    #[test]
    fn multiple_regular_files_are_summed_without_reading_content() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("a"), [0; 3]).unwrap();
        fs::write(fixture.0.join("b"), [0; 4]).unwrap();
        let mut updates = 0;
        let result = scan(
            vec![fixture.0.join("a"), fixture.0.join("b")],
            &AtomicBool::new(false),
            |_| false,
            |_| updates += 1,
        );
        assert_eq!((result.bytes, result.files, updates), (7, 2, 1));
    }
    #[cfg(windows)]
    #[test]
    fn junction_cycle_is_skipped_instead_of_followed() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("a"), [0; 3]).unwrap();
        let junction = fixture.0.join("loop");
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&fixture.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report = scan(
            vec![fixture.0.clone()],
            &AtomicBool::new(false),
            |_| false,
            |_| {},
        );
        assert_eq!((report.bytes, report.files, report.skipped), (3, 1, 1));
        let report = scan(
            vec![junction.clone()],
            &AtomicBool::new(false),
            |_| false,
            |_| {},
        );
        assert_eq!((report.bytes, report.skipped), (0, 1));
        fs::remove_dir(junction).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlink_cycle_and_link_to_external_tree_are_skipped() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("a"), [0; 3]).unwrap();
        std::os::unix::fs::symlink(&fixture.0, fixture.0.join("loop")).unwrap();
        let result = scan(
            vec![fixture.0.clone()],
            &AtomicBool::new(false),
            |_| false,
            |_| {},
        );
        assert_eq!((result.bytes, result.files, result.skipped), (3, 1, 1));
    }
}
