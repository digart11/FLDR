//! Viewport-only automatic policy; unidentified fixed media follows the HDD gate.
use crate::settings::Settings;
use crate::{
    file_entry::{FileEntry, FileKind},
    folder_stats::MAX_REQUEST_PATHS,
};
use std::path::{Path, PathBuf};

/// Views with a live file-cell viewport. Rendering reports rows, never starts IO.
pub fn viewport_enabled(mode: &crate::settings::ViewMode) -> bool {
    matches!(mode, crate::settings::ViewMode::Details | crate::settings::ViewMode::List | crate::settings::ViewMode::Grid)
}

/// Exclude overscan, group headings and parent controls before folder admission.
pub fn viewport_rows(cells: &[usize], range: std::ops::Range<usize>, intersects: impl Fn(usize) -> bool) -> Vec<usize> {
    range.filter(|pos| intersects(*pos)).filter_map(|pos| cells.get(pos).copied())
        .filter(|index| !crate::grouping::is_virtual_cell(*index)).collect()
}

/// Bounded viewport snapshot; roots may expose eligible children, never themselves.
pub fn visible_folders(directory: &Path, entries: &[FileEntry], rows: &[usize]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    rows.iter()
        .filter_map(|i| entries.get(*i))
        .filter(|e| e.kind() == FileKind::Dir && !e.is_symlink() && !e.is_cloud_placeholder())
        .map(FileEntry::path)
        .filter(|p| p.parent() == Some(directory) && !protected_path(p))
        .filter(|p| seen.insert(crate::folder_stats::key(p)))
        .take(MAX_REQUEST_PATHS)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageKind {
    Ssd,
    Hdd,
    Network,
    /// Known local fixed volume, but its physical medium could not be determined.
    FixedUnknown,
    Unknown,
}

pub fn enabled(settings: &Settings, kind: StorageKind) -> bool {
    settings.automatic_folder_statistics
        && match kind {
            StorageKind::Ssd => settings.folder_statistics_ssd,
            StorageKind::Hdd | StorageKind::FixedUnknown => settings.folder_statistics_hdd,
            StorageKind::Network => settings.folder_statistics_network,
            StorageKind::Unknown => false,
        }
}

/// Only SSDs opt into two concurrent child traversals. Potentially seeking or
/// remote media stay serial, even when explicitly enabled.
pub fn concurrency(kind: StorageKind) -> usize {
    if kind == StorageKind::Ssd {
        2
    } else {
        1
    }
}
/// Windows system names at a filesystem/share root, not arbitrary project names.
/// Pure lexical check; actual configured OS paths are additionally checked on workers.
pub fn protected_path(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Component;
        let normalized = crate::folder_stats::key(path);
        let mut components = normalized.components();
        if !matches!(components.next(), Some(Component::Prefix(_)))
            || !matches!(components.next(), Some(Component::RootDir))
        {
            return false;
        }
        let Some(Component::Normal(name)) = components.next() else {
            return false;
        };
        let Some(name) = name.to_str() else {
            return false;
        };
        return [
            "Windows",
            "Program Files",
            "Program Files (x86)",
            "ProgramData",
            "System Volume Information",
            "Recovery",
            "$Recycle.Bin",
            "RECYCLER",
            "RECYCLED",
            "Config.Msi",
            "$Windows.~BT",
            "$Windows.~WS",
        ]
        .iter()
        .any(|blocked| name.eq_ignore_ascii_case(blocked));
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// A failed seek-property query does not make a known fixed volume an SSD.
pub fn fixed_storage(seek: Option<bool>) -> StorageKind {
    match seek {
        Some(false) => StorageKind::Ssd,
        Some(true) => StorageKind::Hdd,
        None => StorageKind::FixedUnknown,
    }
}
/// Bounded in-memory debounce, independent of viewport lifetime. A timer must
/// revisit ready entries; merely consulting this policy never schedules IO.
#[derive(Default)]
pub struct RetryWindow {
    after: std::collections::HashMap<PathBuf, std::time::Instant>,
}
impl RetryWindow {
    pub fn defer(&mut self, paths: &[PathBuf], now: std::time::Instant) {
        for path in paths {
            if self.after.len() >= 512 {
                if let Some(oldest) = self
                    .after
                    .iter()
                    .min_by_key(|(_, time)| **time)
                    .map(|(path, _)| path.clone())
                {
                    self.after.remove(&oldest);
                }
            }
            self.after.insert(
                crate::folder_stats::key(path),
                now + std::time::Duration::from_secs(5),
            );
        }
    }
    pub fn ready(&self, path: &Path, now: std::time::Instant) -> bool {
        self.after
            .get(&crate::folder_stats::key(path))
            .is_none_or(|time| *time <= now)
    }
}

/// Existing 30-second classification cache, with one query reservation and
/// device-change generations. A stale query must release its reservation but
/// can never reauthorize paths after a remount.
#[derive(Default)]
pub struct Classifications {
    entries: std::collections::HashMap<PathBuf, (StorageKind, std::time::Instant)>,
    pending: Option<(PathBuf, u64)>,
    generation: u64,
}
impl Classifications {
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn get(&self, path: &Path, now: std::time::Instant) -> Option<StorageKind> {
        self.entries
            .get(&crate::folder_stats::key(path))
            .filter(|(_, at)| {
                now.saturating_duration_since(*at) < std::time::Duration::from_secs(30)
            })
            .map(|(kind, _)| *kind)
    }
    pub fn known(&self, path: &Path) -> Option<StorageKind> {
        self.entries
            .get(&crate::folder_stats::key(path))
            .map(|(kind, _)| *kind)
    }
    pub fn begin(&mut self, path: &Path) -> Option<u64> {
        if self.pending.is_some() {
            return None;
        }
        self.pending = Some((crate::folder_stats::key(path), self.generation));
        Some(self.generation)
    }
    pub fn complete(
        &mut self,
        path: &Path,
        kind: StorageKind,
        generation: u64,
        now: std::time::Instant,
    ) -> bool {
        let key = crate::folder_stats::key(path);
        if self.pending.as_ref() != Some(&(key.clone(), generation)) {
            return false;
        }
        self.pending = None;
        if self.generation != generation {
            return false;
        }
        if self.entries.len() >= 512 && !self.entries.contains_key(&key) {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, time))| *time)
                .map(|(p, _)| p.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(key, (kind, now));
        true
    }
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.entries.clear();
    }
}

/// Allow one debounce interval for a second cold SSD classification, without
/// letting a slow device query starve a sibling that is already eligible.
#[derive(Default)]
pub struct BatchWindow {
    waiting: Option<(PathBuf, std::time::Instant)>,
}
impl BatchWindow {
    pub fn ready(
        &mut self,
        first: &Path,
        known: usize,
        capacity: usize,
        query_pending: bool,
        now: std::time::Instant,
    ) -> bool {
        if capacity < 2 || known >= 2 || !query_pending {
            self.waiting = None;
            return true;
        }
        let first = crate::folder_stats::key(first);
        match &self.waiting {
            Some((path, until)) if *path == first && *until <= now => {
                self.waiting = None;
                true
            }
            Some((path, _)) if *path == first => false,
            _ => {
                self.waiting = Some((first, now + std::time::Duration::from_millis(300)));
                false
            }
        }
    }
}

/// Viewport repainting alone is not a reason to keep the automatic timer alive.
#[derive(Default)]
pub struct ObservationWake {
    settings: Option<[bool; 4]>,
}
impl ObservationWake {
    pub fn update(&mut self, changed: bool, running: bool, settings: &Settings) -> bool {
        let stamp = [
            settings.automatic_folder_statistics,
            settings.folder_statistics_ssd,
            settings.folder_statistics_hdd,
            settings.folder_statistics_network,
        ];
        let settings_changed = self.settings.replace(stamp) != Some(stamp);
        running || (settings.automatic_folder_statistics && (changed || settings_changed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_migration_and_each_storage_gate() {
        let mut settings: Settings = serde_json::from_str(
            r#"{"show_hidden":true,"toolbar_order":["paste","split","copy"]}"#,
        )
        .unwrap();
        assert!(settings.show_hidden);
        assert_eq!(settings.toolbar_order, ["paste", "split", "copy"]);
        assert!(enabled(&settings, StorageKind::Ssd));
        for kind in [StorageKind::Hdd, StorageKind::Network, StorageKind::Unknown] {
            assert!(!enabled(&settings, kind));
        }
        settings.folder_statistics_hdd = true;
        settings.folder_statistics_network = true;
        for kind in [StorageKind::Ssd, StorageKind::Hdd, StorageKind::Network] {
            assert!(enabled(&settings, kind));
        }
        assert!(!enabled(&settings, StorageKind::Unknown));
        settings.folder_statistics_ssd = false;
        assert!(!enabled(&settings, StorageKind::Ssd));
        assert!(enabled(&settings, StorageKind::Hdd));
        assert!(enabled(&settings, StorageKind::Network));
        settings.automatic_folder_statistics = false;
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        for kind in [
            StorageKind::Ssd,
            StorageKind::Hdd,
            StorageKind::Network,
            StorageKind::Unknown,
        ] {
            assert!(!enabled(&restored, kind));
        }
        assert!(restored.show_hidden && restored.folder_statistics_network);
    }
    #[test]
    fn device_changes_reject_old_classifications_without_duplicating_queries() {
        let now = std::time::Instant::now();
        let path = Path::new("fixture/a");
        let mut cache = Classifications::default();
        assert_eq!(cache.get(path, now), None);
        let old = cache.begin(path).unwrap();
        assert!(cache.begin(Path::new("fixture/b")).is_none());
        cache.invalidate();
        assert!(cache.begin(path).is_none()); // the old OS call still occupies its slot
        assert!(!cache.complete(path, StorageKind::Ssd, old, now));
        assert_eq!(cache.known(path), None);
        let new = cache.begin(path).unwrap();
        assert_ne!(new, old);
        assert!(cache.complete(path, StorageKind::FixedUnknown, new, now));
        assert_eq!(cache.get(path, now), Some(StorageKind::FixedUnknown));
        assert_eq!(
            cache.get(path, now + std::time::Duration::from_secs(30)),
            None
        );
        assert_eq!(cache.known(path), Some(StorageKind::FixedUnknown));
        assert!(!enabled(&Settings::default(), cache.known(path).unwrap()));
        for i in 0..1000 {
            let path = PathBuf::from(format!("fixture/{i}"));
            let ticket = cache.begin(&path).unwrap();
            assert!(cache.complete(&path, StorageKind::Hdd, ticket, now));
        }
        assert_eq!(cache.entries.len(), 512);
    }
    #[test]
    fn unchanged_idle_viewports_do_not_rearm_but_settings_and_active_cancellation_do() {
        let mut wake = ObservationWake::default();
        let mut s = Settings::default();
        assert!(wake.update(true, false, &s));
        for _ in 0..100 {
            assert!(!wake.update(false, false, &s));
        }
        s.folder_statistics_hdd = !s.folder_statistics_hdd;
        assert!(wake.update(false, false, &s));
        assert!(!wake.update(false, false, &s));
        s.automatic_folder_statistics = false;
        assert!(wake.update(false, true, &s)); // cancel running work
        assert!(!wake.update(true, false, &s));
        s.automatic_folder_statistics = true;
        assert!(wake.update(false, false, &s));
    }
    #[test]
    fn cold_ssd_batching_waits_briefly_but_never_starves_a_known_candidate() {
        let now = std::time::Instant::now();
        let path = Path::new("fixture/first");
        let mut batch = BatchWindow::default();
        assert!(!batch.ready(path, 1, 2, true, now));
        assert!(batch.ready(path, 2, 2, false, now)); // second classification arrived
        assert!(!batch.ready(path, 1, 2, true, now));
        assert!(batch.ready(
            path,
            1,
            2,
            true,
            now + std::time::Duration::from_millis(300)
        )); // blocked query cannot starve first
        assert!(batch.ready(path, 1, 1, true, now)); // seeking/remote media never wait for parallel work
        assert!(batch.ready(path, 1, 2, false, now)); // only one visible eligible directory
    }
    #[test]
    fn fixed_unknown_never_bypasses_media_or_network_gates() {
        for master in [false, true] {
            for ssd in [false, true] {
                for hdd in [false, true] {
                    for net in [false, true] {
                        let mut s = Settings::default();
                        s.automatic_folder_statistics = master;
                        s.folder_statistics_ssd = ssd;
                        s.folder_statistics_hdd = hdd;
                        s.folder_statistics_network = net;
                        assert_eq!(enabled(&s, fixed_storage(None)), master && hdd);
                        assert_eq!(enabled(&s, fixed_storage(Some(false))), master && ssd);
                        assert_eq!(enabled(&s, fixed_storage(Some(true))), master && hdd);
                        assert_eq!(enabled(&s, StorageKind::Network), master && net);
                        assert!(!enabled(&s, StorageKind::Unknown));
                    }
                }
            }
        }
        assert_eq!(concurrency(StorageKind::Ssd), 2);
        for kind in [
            StorageKind::Hdd,
            StorageKind::FixedUnknown,
            StorageKind::Network,
            StorageKind::Unknown,
        ] {
            assert_eq!(concurrency(kind), 1);
        }
    }
    #[test]
    fn cancelled_or_invalidated_work_retries_after_a_bounded_navigation_independent_delay() {
        let now = std::time::Instant::now();
        let path = PathBuf::from("fixture/child");
        let mut retry = RetryWindow::default();
        assert!(retry.ready(&path, now));
        retry.defer(&[path.clone()], now);
        assert!(!retry.ready(&path, now + std::time::Duration::from_secs(4)));
        assert!(retry.ready(&path, now + std::time::Duration::from_secs(5)));
        // Navigation changes neither the deadline nor any scanner reservation.
        for _ in 0..100 {
            assert!(retry.ready(Path::new("elsewhere"), now));
        }
        for i in 0..1000 {
            retry.defer(&[PathBuf::from(format!("fixture/{i}"))], now);
        }
        assert_eq!(retry.after.len(), 512);
        assert!(retry
            .after
            .keys()
            .all(|p| retry.ready(p, now + std::time::Duration::from_secs(5))));
    }
}
