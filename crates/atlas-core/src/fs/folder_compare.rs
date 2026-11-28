//! Compare two retained immediate-directory listings on a worker.
use atlas_model::file_entry::{FileEntry, FileKind};
use atlas_model::folder_compare::{Comparison, Mark};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn compare(
    left: &[FileEntry],
    right: &[FileEntry],
    verify: bool,
    cancel: &AtomicBool,
) -> Comparison {
    let mut result = Comparison::default();
    let mut right_by_name: HashMap<OsString, _> = HashMap::with_capacity(right.len());
    for entry in right {
        if cancel.load(Ordering::Relaxed) {
            return Comparison::default();
        }
        let path = entry.path();
        if let Some(name) = path.file_name() {
            right_by_name.insert(name.to_os_string(), (entry, path.clone()));
        }
    }
    let mut buffer = if verify {
        vec![0; 128 * 1024]
    } else {
        Vec::new()
    };
    for a in left {
        if cancel.load(Ordering::Relaxed) {
            return Comparison::default();
        }
        let path_a = a.path();
        let Some(name) = path_a.file_name() else {
            continue;
        };
        match right_by_name.remove(name) {
            Some((b, path_b)) => {
                let mut mark = compare_metadata(a, b);
                if verify
                    && a.kind() == FileKind::File
                    && b.kind() == FileKind::File
                    && a.size().is_some()
                    && a.size() == b.size()
                {
                    let hashes = digest(&path_a, a, &mut buffer, cancel).zip(digest(
                        &path_b,
                        b,
                        &mut buffer,
                        cancel,
                    ));
                    mark = match hashes {
                        Some((a, b)) if a == b => Mark::IdenticalContent,
                        Some(_) => Mark::Different,
                        None if matches!(mark, Mark::Different) => Mark::Different,
                        None => Mark::Unverified,
                    };
                }
                result.left.insert(path_a, mark);
                result.right.insert(path_b, mark);
            }
            None => {
                result.left.insert(path_a, Mark::LeftOnly);
            }
        }
    }
    for (_, (_, path)) in right_by_name {
        if cancel.load(Ordering::Relaxed) {
            return Comparison::default();
        }
        result.right.insert(path, Mark::RightOnly);
    }
    result
}

fn compare_metadata(a: &FileEntry, b: &FileEntry) -> Mark {
    if a.kind() != b.kind()
        || a.size().zip(b.size()).is_some_and(|(a, b)| a != b)
        || a.modified.zip(b.modified).is_some_and(|(a, b)| a != b)
    {
        Mark::Different
    } else if a.modified.is_some()
        && b.modified.is_some()
        && (a.kind() == FileKind::Dir || a.size().zip(b.size()).is_some())
    {
        Mark::MetadataMatch
    } else {
        Mark::Unverified
    }
}

fn digest(
    path: &Path,
    entry: &FileEntry,
    buffer: &mut [u8],
    cancel: &AtomicBool,
) -> Option<[u8; 32]> {
    if cancel.load(Ordering::Relaxed) || super::preview::is_cloud_placeholder(path) {
        return None;
    }
    let expected = entry.size()?;
    let modified = entry.modified?;
    let matches = |meta: &std::fs::Metadata| {
        meta.is_file() && meta.len() == expected && meta.modified().ok() == Some(modified)
    };
    if !matches(&std::fs::symlink_metadata(path).ok()?) {
        return None;
    }

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Do not follow a reparse point swapped in after enumeration.
        options.custom_flags(0x0020_0000);
    }
    let mut file: File = options.open(path).ok()?;
    let opened = file.metadata().ok()?;
    if !matches(&opened) {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if opened.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    let mut hash = Sha256::new();
    let mut read = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let n = file.read(buffer).ok()?;
        if n == 0 {
            break;
        }
        read = read.checked_add(n as u64)?;
        if read > expected {
            return None;
        }
        hash.update(&buffer[..n]);
    }
    (read == expected && matches(&file.metadata().ok()?)).then(|| hash.finalize().into())
}
