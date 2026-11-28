//! Cache-only selection totals; selection never starts a recursive scan.
use crate::{
    file_entry::{FileEntry, FileKind},
    folder_stats::{key, Cache, SizeCell},
};
use std::time::Instant;

#[derive(Debug, PartialEq, Eq)]
pub struct SelectionTotal {
    pub bytes: u64,
    pub complete: bool,
    pub cached: bool,
}
impl SelectionTotal {
    pub fn show(&self) -> bool {
        self.complete || self.bytes > 0
    }
}
pub fn total(entries: &[FileEntry], cache: &Cache, now: Instant) -> SelectionTotal {
    let mut paths: Vec<_> = entries.iter().map(|e| (key(&e.path()), e)).collect();
    paths.sort_by_key(|(path, _)| path.components().count());
    let mut directories = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    let mut out = SelectionTotal {
        bytes: 0,
        complete: true,
        cached: false,
    };
    for (path, entry) in paths {
        if !seen.insert(path.clone()) || path.ancestors().skip(1).any(|p| directories.contains(p)) {
            continue;
        }
        let value = match entry.kind() {
            FileKind::Dir => {
                directories.insert(path.clone());
                match cache.cell(&path, now) {
                    SizeCell::Exact(n) => Some(n),
                    SizeCell::Cached(n) => {
                        out.cached = true;
                        Some(n)
                    }
                    SizeCell::Partial(n) => {
                        out.complete = false;
                        Some(n)
                    }
                    _ => {
                        out.complete = false;
                        None
                    }
                }
            }
            FileKind::File => {
                if entry.size().is_none() {
                    out.complete = false;
                }
                entry.size()
            }
            _ => {
                out.complete = false;
                None
            }
        };
        if let Some(value) = value {
            match out.bytes.checked_add(value) {
                Some(sum) => out.bytes = sum,
                None => {
                    out.bytes = u64::MAX;
                    out.complete = false;
                }
            }
        }
    }
    out
}
