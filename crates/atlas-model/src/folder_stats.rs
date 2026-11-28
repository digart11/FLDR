//! Shared, bounded directory snapshots with optional background persistence. Reading the cache never schedules IO.
use crate::folder_size::SizeReport;
use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

pub const CACHE_CAPACITY: usize = 512;
pub const REFRESH_AGE: Duration = Duration::from_secs(15 * 60);
pub const DISK_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const MAX_REQUEST_PATHS: usize = 128;
/// Age for subdued presentation, not an expiration or invalidation deadline.
pub const MAX_AGE: Duration = Duration::from_secs(60);

/// Lexical filesystem key; no filesystem access or lossy Unicode conversion.
pub fn key(path: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let mut wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .map(|c| if c == b'/' as u16 { b'\\' as u16 } else { c })
            .collect();
        let prefix: Vec<u16> = "\\\\?\\unc\\".encode_utf16().collect();
        if wide.get(..prefix.len()).is_some_and(|p| {
            p.iter().zip(&prefix).all(|(a, b)| {
                (if (b'A' as u16..=b'Z' as u16).contains(a) {
                    *a + 32
                } else {
                    *a
                }) == *b
            })
        }) {
            wide.splice(..prefix.len(), "\\\\".encode_utf16());
        } else if wide.starts_with(&"\\\\?\\".encode_utf16().collect::<Vec<_>>())
            && wide.get(5) == Some(&(b':' as u16))
        {
            wide.drain(..4);
        }
        // Preserve component case: NTFS directories and SMB servers can be case
        // sensitive. Only the drive letter has universally case-insensitive identity.
        if wide.get(1) == Some(&(b':' as u16)) && (b'A' as u16..=b'Z' as u16).contains(&wide[0]) {
            wide[0] += 32;
        }
        PathBuf::from(std::ffi::OsString::from_wide(&wide))
    };
    #[cfg(not(windows))]
    let path = path.to_path_buf();
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(part.as_os_str()),
        }
    }
    normalized
}
fn related(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Unknown,
    Calculating,
    Completed,
    Partial,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct DirectoryStats {
    pub path: PathBuf,
    pub report: SizeReport,
    pub state: State,
    pub calculated_at: SystemTime,
    pub observed_at: Instant,
}
impl DirectoryStats {
    pub fn new(path: PathBuf, report: SizeReport) -> Self {
        let state = if !report.done {
            State::Calculating
        } else if report.cancelled {
            State::Cancelled
        } else if report.errors > 0 && report.files == 0 && report.folders == 0 {
            State::Failed
        } else if report.errors > 0 || report.skipped > 0 {
            State::Partial
        } else {
            State::Completed
        };
        Self {
            path,
            report,
            state,
            calculated_at: SystemTime::now(),
            observed_at: Instant::now(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Bytes,
    Folders,
    Files,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum SizeCell {
    Unknown,
    Calculating,
    Exact(u64),
    /// Resident snapshot past the freshness interval; usable, not invalidated.
    Cached(u64),
    Partial(u64),
    Failed,
    Cancelled,
    Stale,
}
#[derive(Clone, Copy, Debug)]
pub struct Ticket {
    pub generation: u64,
    pub change_epoch: u64,
}
pub struct Entry {
    pub stats: DirectoryStats,
    pub generation: u64,
    pub invalidated: bool,
    completed: Instant,
    aged: bool,
    refresh_due: bool,
    retry_after: Option<Instant>,
    used: Cell<u64>,
    pinned: bool,
    requested: bool,
    depth: usize,
}
pub struct Cache {
    entries: HashMap<PathBuf, Entry>,
    /// At most MAX_REQUEST_PATHS prior snapshots during an automatic refresh.
    fallbacks: HashMap<PathBuf, Entry>,
    capacity: usize,
    ttl: Duration,
    generation: u64,
    active: Option<u64>,
    roots: Vec<PathBuf>,
    visible: HashSet<PathBuf>,
    automatic: bool,
    automatic_slots: usize,
    epoch: u64,
    changes: VecDeque<(u64, Option<PathBuf>)>,
    clock: Cell<u64>,
    pub revision: u64,
}
impl Default for Cache {
    fn default() -> Self {
        Self::new(CACHE_CAPACITY, MAX_AGE)
    }
}
impl Cache {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            fallbacks: HashMap::new(),
            capacity: capacity.max(MAX_REQUEST_PATHS),
            ttl,
            generation: 0,
            active: None,
            roots: Vec::new(),
            visible: HashSet::new(),
            automatic: false,
            automatic_slots: capacity.max(MAX_REQUEST_PATHS),
            epoch: 0,
            changes: VecDeque::new(),
            clock: Cell::new(0),
            revision: 0,
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    fn touch(&self) -> u64 {
        let next = self.clock.get().wrapping_add(1);
        self.clock.set(next);
        next
    }
    fn make_room(
        &mut self,
        path: &Path,
        depth: Option<usize>,
        requested: bool,
        force_visible: bool,
    ) -> bool {
        if self.entries.contains_key(path) || self.entries.len() < self.capacity {
            return true;
        }
        let victim = self
            .entries
            .iter()
            .filter(|(cached, entry)| {
                !entry.pinned
                    && (force_visible || !self.visible.contains(*cached))
                    && (requested || !entry.requested)
            })
            .min_by_key(|(cached, entry)| {
                let current = entry.generation == self.generation && self.active.is_some();
                (
                    self.visible.contains(*cached),
                    entry.requested,
                    current,
                    std::cmp::Reverse(if current { entry.depth } else { 0 }),
                    entry.used.get(),
                )
            })
            .map(|(path, _)| path.clone());
        if let Some(victim) = victim {
            if let Some(depth) = depth {
                let entry = &self.entries[&victim];
                if entry.generation == self.generation && depth > entry.depth {
                    return false;
                }
            }
            self.entries.remove(&victim);
            true
        } else {
            false
        }
    }
    /// Protect a bounded visible working set. Extra panes/rows outside this budget
    /// cannot trigger automatic churn; reprioritize them on navigation/focus change.
    pub fn set_visible(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.automatic_slots = self.capacity;
        self.update_visible(paths);
    }
    /// Result-driven sorting may change visible identities without user movement.
    /// Update protection without replenishing that unchanged viewport's admission budget.
    pub fn update_visible(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.visible.clear();
        for path in paths {
            self.visible.insert(key(&path));
            if self.visible.len() >= self.capacity {
                break;
            }
        }
    }
    pub fn automatic_candidate(&self, path: &Path, now: Instant) -> bool {
        self.automatic_slots > 0
            && self.visible.contains(&key(path))
            && self.entry(path).is_none_or(|e| e.retry_after.is_none_or(|at| at <= now))
            && (matches!(self.cell(path, now), SizeCell::Unknown | SizeCell::Stale)
                || self.entry(path).is_some_and(|e| e.stats.state == State::Completed
                    && now.saturating_duration_since(e.completed) >= REFRESH_AGE))
    }
    pub fn automatic_slots(&self) -> usize {
        self.automatic_slots
    }
    /// Explicit manual recalculation invalidates related older snapshots.
    pub fn begin(&mut self, paths: &[PathBuf], now: Instant) -> Option<Ticket> {
        self.begin_request(paths, now, false)
    }
    /// Computing an unknown/stale visible child is not a filesystem mutation.
    pub fn begin_automatic(&mut self, paths: &[PathBuf], now: Instant) -> Option<Ticket> {
        if paths.is_empty()
            || paths.len() > self.automatic_slots
            || paths.iter().any(|p| !self.automatic_candidate(p, now))
        {
            return None;
        }
        self.begin_request(paths, now, true)
    }
    fn begin_request(
        &mut self,
        paths: &[PathBuf],
        now: Instant,
        automatic: bool,
    ) -> Option<Ticket> {
        if self.active.is_some() || paths.len() > MAX_REQUEST_PATHS {
            return None;
        }
        let paths: HashSet<_> = paths.iter().map(|p| key(p)).collect();
        if paths.iter().any(|p| {
            self.entries
                .get(p)
                .is_some_and(|e| e.stats.state == State::Calculating)
        }) {
            return None;
        }
        // Explicit recalculation makes related older snapshots suspect. In
        // particular, a refreshed child must not leave an older parent exact.
        if !automatic {
            for path in &paths {
                self.invalidate(Some(path));
            }
        }
        self.automatic = automatic;
        self.roots = paths.iter().cloned().collect();
        self.generation += 1;
        self.active = Some(self.generation);
        let ticket = Ticket {
            generation: self.generation,
            change_epoch: self.epoch,
        };
        for path in paths {
            if !self.make_room(&path, None, true, !automatic) {
                return None;
            }
            if automatic && self.entries.get(&path).is_some_and(|e|
                matches!(e.stats.state, State::Completed | State::Partial)) {
                if let Some(previous) = self.entries.remove(&path) { self.fallbacks.insert(path.clone(), previous); }
            }
            let used = self.touch();
            self.entries.insert(
                path.clone(),
                Entry {
                    stats: DirectoryStats::new(path, SizeReport::default()),
                    generation: ticket.generation,
                    invalidated: false,
                    completed: now,
                    aged: false,
                    refresh_due: false,
                    retry_after: None,
                    used: Cell::new(used),
                    pinned: true,
                    requested: true,
                    depth: 0,
                },
            );
        }
        if automatic {
            self.automatic_slots -= self.roots.len();
        }
        self.revision += 1;
        Some(ticket)
    }
    pub fn publish(&mut self, ticket: Ticket, mut stats: DirectoryStats, now: Instant) -> bool {
        if self.active != Some(ticket.generation) {
            return false;
        }
        let path = key(&stats.path);
        stats.path = path.clone();
        if self.entries.get(&path).is_some_and(|entry| {
            entry.generation > ticket.generation
                || (entry.generation == ticket.generation && entry.stats.report.done)
        }) {
            return false;
        }
        // An incidental failed/cancelled automatic subtree must not replace a
        // valid completed snapshot (including a prior explicit manual result).
        if self.automatic
            && stats.state != State::Completed
            && self
                .entries
                .get(&path)
                .is_some_and(|entry| !entry.invalidated && entry.stats.state == State::Completed)
        {
            return false;
        }
        let depth = relative_depth(&path, self.roots.iter());
        if !self.make_room(&path, Some(depth), false, false) {
            return false;
        }
        let pinned = self.entries.get(&path).is_some_and(|entry| entry.pinned);
        let requested = self.entries.get(&path).is_some_and(|entry| entry.requested);
        let invalidated = self
            .changes
            .front()
            .is_some_and(|(epoch, _)| ticket.change_epoch + 1 < *epoch)
            || self.changes.iter().any(|(epoch, changed)| {
                *epoch > ticket.change_epoch
                    && changed
                        .as_ref()
                        .is_none_or(|changed| related(&path, changed))
            });
        let used = self.touch();
        let completed = now.min(stats.observed_at);
        self.entries.insert(
            path,
            Entry {
                stats,
                generation: ticket.generation,
                invalidated,
                completed,
                aged: false,
                    refresh_due: false,
                    retry_after: None,
                used: Cell::new(used),
                pinned,
                requested,
                depth,
            },
        );
        self.revision += 1;
        true
    }
    pub fn entry(&self, path: &Path) -> Option<&Entry> {
        let entry = self.entries.get(&key(path))?;
        entry.used.set(self.touch());
        Some(entry)
    }
    pub fn cell(&self, path: &Path, now: Instant) -> SizeCell {
        self.metric_cell(path, Metric::Bytes, now)
    }
    pub fn metric_cell(&self, path: &Path, metric: Metric, now: Instant) -> SizeCell {
        let Some(entry) = self.entry(path) else {
            return SizeCell::Unknown;
        };
        if entry.stats.state == State::Calculating {
            return SizeCell::Calculating;
        }
        if entry.invalidated {
            return SizeCell::Stale;
        }
        let value = match metric {
            Metric::Bytes => entry.stats.report.bytes,
            Metric::Folders => entry.stats.report.folders,
            Metric::Files => entry.stats.report.files,
        };
        match entry.stats.state {
            State::Unknown => SizeCell::Unknown,
            State::Calculating => SizeCell::Calculating,
            State::Completed if now.saturating_duration_since(entry.completed) >= self.ttl => {
                SizeCell::Cached(value)
            }
            State::Completed => SizeCell::Exact(value),
            State::Partial => SizeCell::Partial(value),
            State::Failed => SizeCell::Failed,
            State::Cancelled => SizeCell::Cancelled,
        }
    }
    /// Complete snapshot bytes for ordering, including aged cached values.
    /// Known-invalidated and partial results are never returned.
    pub fn exact_bytes(&self, path: &Path, now: Instant) -> Option<u64> {
        match self.cell(path, now) {
            SizeCell::Exact(bytes) | SizeCell::Cached(bytes) => Some(bytes),
            _ => None,
        }
    }
    /// Scope invalidation covers known ancestors and descendants (rename/delete/overflow).
    /// None is conservative invalidation for operation messages without path information.
    pub fn invalidate(&mut self, path: Option<&Path>) {
        self.automatic_slots = self.capacity;
        self.epoch += 1;
        let path = path.map(key);
        for (cached, entry) in self.entries.iter_mut().chain(self.fallbacks.iter_mut()) {
            if path.as_ref().is_none_or(|path| related(cached, path)) {
                entry.invalidated = true;
                entry.retry_after = None;
            }
        }
        self.changes.push_back((self.epoch, path));
        while self.changes.len() > 128 {
            self.changes.pop_front();
        }
        self.revision += 1;
    }
    /// Request a repaint at the freshness boundary without invalidating snapshots.
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for entry in self.entries.values_mut() {
            if entry.retry_after.is_some_and(|at| at <= now) {
                entry.retry_after = None;
                self.automatic_slots = self.capacity;
                changed = true;
            }
            if entry.stats.state == State::Completed && !entry.refresh_due
                && now.saturating_duration_since(entry.completed) >= REFRESH_AGE {
                entry.refresh_due = true;
                self.automatic_slots = self.capacity;
                changed = true;
            }
            if entry.stats.report.done
                && !entry.invalidated
                && !entry.aged
                && now.saturating_duration_since(entry.completed) >= self.ttl
            {
                entry.aged = true;
                changed = true;
            }
        }
        if changed {
            self.revision += 1;
        }
        changed
    }
    pub fn next_expiry(&self, now: Instant) -> Option<Duration> {
        self.entries
            .values()
            .filter(|e| e.stats.report.done && (e.retry_after.is_some() || (!e.invalidated && (!e.aged ||
                (e.stats.state == State::Completed && !e.refresh_due)))))
            .map(|e| e.retry_after.map_or_else(|| {
                (if e.aged { REFRESH_AGE } else { self.ttl })
                    .saturating_sub(now.saturating_duration_since(e.completed))
            }, |at| at.saturating_duration_since(now)))
            .min()
    }
    pub fn finish_missing(&mut self, ticket: Ticket, cancelled: bool, now: Instant) {
        for entry in self
            .entries
            .values_mut()
            .filter(|e| e.generation == ticket.generation)
        {
            entry.pinned = false;
            if !entry.stats.report.done {
                entry.stats.report.done = true;
                entry.stats.report.cancelled = cancelled;
                entry.stats.state = if cancelled {
                    State::Cancelled
                } else {
                    State::Failed
                };
                entry.completed = now;
            }
        }
        if self.active == Some(ticket.generation) {
            for (path, mut previous) in self.fallbacks.drain() {
                if let Some(current) = self.entries.get(&path).filter(|e|
                    matches!(e.stats.state, State::Cancelled | State::Failed)) {
                    previous.invalidated |= current.invalidated;
                    if current.stats.state == State::Failed {
                        // Retain the old cached value without turning a failed
                        // refresh into a five-second scan loop.
                        previous.retry_after = Some(now + REFRESH_AGE);
                        previous.aged = true;
                        previous.refresh_due = true;
                    }
                    previous.pinned = false;
                    self.entries.insert(path, previous);
                }
            }
        }
        self.revision += 1;
        if self.active == Some(ticket.generation) {
            self.active = None;
            self.roots.clear();
        }
    }
    /// Retire every unfinished snapshot of an obsolete automatic batch, including
    /// open descendants. Completed children and unrelated/manual generations stay.
    pub fn retry_cancelled_scan(&mut self, ticket: Ticket) {
        let before = self.entries.len();
        self.entries.retain(|_, entry| {
            entry.generation != ticket.generation || entry.stats.state != State::Cancelled
        });
        if self.entries.len() != before {
            self.revision += 1;
        }
    }
    /// Obsolete automatic roots can be requested when visible again. Keep all
    /// completed descendants, including results obtained before cancellation.
    pub fn retry_cancelled(&mut self, path: &Path) {
        let path = key(path);
        if self
            .entries
            .get(&path)
            .is_some_and(|e| e.stats.state == State::Cancelled)
        {
            self.entries.remove(&path);
            self.revision += 1;
        }
    }
}

/// Versioned disk envelope; values contain no filesystem validation hints. A root's
/// timestamp cannot establish recursive freshness, so restored values are cached.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DiskCache {
    pub version: u32,
    pub entries: Vec<DiskEntry>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DiskEntry {
    // Native units preserve unusual Windows filenames without lossy conversion.
    pub path: Vec<u16>,
    pub report: SizeReport,
    pub calculated_at: SystemTime,
    pub invalidated: bool,
}
fn encode_path(path: &Path) -> Option<Vec<u16>> {
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        Some(path.as_os_str().encode_wide().collect())
    }
    #[cfg(not(windows))] { Some(path.to_str()?.encode_utf16().collect()) }
}
fn decode_path(path: &[u16]) -> Option<PathBuf> {
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStringExt;
        Some(std::ffi::OsString::from_wide(path).into())
    }
    #[cfg(not(windows))] { String::from_utf16(path).ok().map(PathBuf::from) }
}
impl Cache {
    pub fn disk_snapshot(&self, now: SystemTime) -> DiskCache {
        let mut entries: Vec<_> = self.entries.values().chain(self.fallbacks.values().filter(|previous|
            self.entries.get(&previous.stats.path).is_some_and(|current| current.stats.state == State::Calculating)))
            .filter(|e|
            matches!(e.stats.state, State::Completed | State::Partial)
            && now.duration_since(e.stats.calculated_at).is_ok_and(|age| age <= DISK_MAX_AGE))
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.used.get()));
        DiskCache { version: 1, entries: entries.into_iter().take(CACHE_CAPACITY).filter_map(|e|
            Some(DiskEntry { path: encode_path(&e.stats.path)?, report: e.stats.report.clone(),
                calculated_at: e.stats.calculated_at, invalidated: e.invalidated })).collect() }
    }
    /// Merge only absent paths and respect all mutations since asynchronous load began.
    /// Never probes cached paths (including remote shares).
    pub fn restore_disk(&mut self, disk: DiskCache, now: Instant, wall: SystemTime) {
        if disk.version != 1 || disk.entries.len() > CACHE_CAPACITY { return; }
        let available = self.capacity.saturating_sub(self.entries.len());
        for record in disk.entries.into_iter().take(available).collect::<Vec<_>>().into_iter().rev() {
            let Ok(age) = wall.duration_since(record.calculated_at) else { continue; };
            if age > DISK_MAX_AGE || !record.report.done || record.report.cancelled
                || record.path.len() > 32768 { continue; }
            let Some(path) = decode_path(&record.path) else { continue; };
            let path = key(&path);
            if !path.is_absolute() || self.entries.contains_key(&path) || self.entries.len() >= self.capacity { continue; }
            let invalidated = record.invalidated || self.epoch > self.changes.len() as u64
                || self.changes.iter().any(|(_, changed)| changed.as_ref().is_none_or(|p| related(&path, p)));
            let mut stats = DirectoryStats::new(path.clone(), record.report);
            if !matches!(stats.state, State::Completed | State::Partial) { continue; }
            stats.calculated_at = record.calculated_at;
            let completed = now.checked_sub(age.max(self.ttl)).unwrap_or(now);
            let used = self.touch();
            self.entries.insert(path, Entry { stats, generation: 0, invalidated,
                completed, aged: true, refresh_due: false, retry_after: None, used: Cell::new(used),
                pinned: false, requested: false, depth: 0 });
        }
        self.revision += 1;
    }
}

fn relative_depth<'a>(path: &Path, roots: impl Iterator<Item = &'a PathBuf>) -> usize {
    roots
        .filter_map(|root| path.strip_prefix(root).ok().map(|p| p.components().count()))
        .min()
        .unwrap_or(usize::MAX)
}

/// Bounded coalescing mailbox. Requested roots have priority over incidental children.
pub struct Inbox {
    entries: HashMap<PathBuf, DirectoryStats>,
    requested: HashSet<PathBuf>,
}
impl Inbox {
    pub fn new(requested: &[PathBuf]) -> Self {
        Self {
            entries: HashMap::new(),
            requested: requested.iter().map(|p| key(p)).collect(),
        }
    }
    pub fn push(&mut self, stats: DirectoryStats) {
        let path = key(&stats.path);
        if self.entries.len() >= CACHE_CAPACITY && !self.entries.contains_key(&path) {
            let victim = self
                .entries
                .keys()
                .filter(|p| !self.requested.contains(*p))
                .max_by_key(|p| relative_depth(p, self.requested.iter()))
                .cloned();
            if let Some(victim) = victim {
                if relative_depth(&path, self.requested.iter())
                    > relative_depth(&victim, self.requested.iter())
                {
                    return;
                }
                self.entries.remove(&victim);
            } else {
                return;
            }
        }
        self.entries.insert(path, stats);
    }
    pub fn drain(&mut self) -> Vec<DirectoryStats> {
        let mut values: Vec<_> = self.entries.drain().map(|(_, value)| value).collect();
        values.sort_by_key(|s| {
            std::cmp::Reverse(relative_depth(&key(&s.path), self.requested.iter()))
        });
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn completed(path: &Path, bytes: u64, now: Instant) -> DirectoryStats {
        let mut stats = DirectoryStats::new(
            path.to_path_buf(),
            SizeReport {
                bytes,
                files: 1,
                done: true,
                ..Default::default()
            },
        );
        stats.observed_at = now;
        stats
    }
    #[test]
    fn resident_results_survive_selection_navigation_and_ordinary_time() {
        let now = Instant::now();
        let path = PathBuf::from("fixture/a");
        let mut cache = Cache::default();
        let ticket = cache.begin(&[path.clone()], now).unwrap();
        assert_eq!(cache.cell(&path, now), SizeCell::Calculating);
        cache.publish(ticket, completed(&path, 123, now), now);
        cache.finish_missing(ticket, false, now);
        // Cache lookup has no tab, selection or listing-version dependency.
        assert_eq!(cache.cell(&path, now), SizeCell::Exact(123));
        assert_eq!(
            cache.cell(Path::new("fixture/unvisited"), now),
            SizeCell::Unknown
        );
        assert_eq!(
            cache.cell(&path, now + Duration::from_secs(30)),
            SizeCell::Exact(123)
        );
        assert_eq!(cache.cell(&path, now + MAX_AGE), SizeCell::Cached(123));
        assert!(cache.expire(now + MAX_AGE));
        assert_eq!(cache.next_expiry(now + MAX_AGE), Some(REFRESH_AGE - MAX_AGE));
        let later = now + Duration::from_secs(86400);
        assert_eq!(cache.cell(&path, later), SizeCell::Cached(123));
        assert_eq!(cache.exact_bytes(&path, later), Some(123));
        assert!(cache.expire(later));
        assert!(!cache.expire(later));
        cache.invalidate(Some(&path));
        assert_eq!(cache.cell(&path, later), SizeCell::Stale);
        assert_eq!(cache.exact_bytes(&path, later), None);
        let newer = cache.begin(&[path.clone()], later).unwrap();
        cache.publish(newer, completed(&path, 456, later), later);
        cache.finish_missing(newer, false, later);
        assert_eq!(cache.cell(&path, later), SizeCell::Exact(456));
    }
    #[test]
    fn changes_invalidate_ancestors_descendants_and_inflight_snapshots() {
        let now = Instant::now();
        let mut cache = Cache::default();
        let root = PathBuf::from("fixture/root");
        let child = root.join("child");
        let sibling = PathBuf::from("fixture/other");
        let ticket = cache
            .begin(&[root.clone(), child.clone(), sibling.clone()], now)
            .unwrap();
        for p in [&root, &child, &sibling] {
            cache.publish(ticket, completed(p, 10, now), now);
        }
        cache.finish_missing(ticket, false, now);
        cache.invalidate(Some(&child.join("nested/changed.txt")));
        assert_eq!(cache.cell(&root, now), SizeCell::Stale);
        assert_eq!(cache.cell(&child, now), SizeCell::Stale);
        assert_eq!(cache.cell(&sibling, now), SizeCell::Exact(10));
        let next = cache.begin(&[child.clone()], now).unwrap();
        cache.invalidate(Some(&root));
        cache.publish(next, completed(&child, 99, now), now);
        cache.finish_missing(next, false, now);
        assert_eq!(cache.cell(&child, now), SizeCell::Stale);
        cache.invalidate(None);
        assert_eq!(cache.cell(&sibling, now), SizeCell::Stale);
    }
    #[test]
    fn concurrent_duplicate_requests_share_one_reservation() {
        let cache = std::sync::Arc::new(std::sync::Mutex::new(Cache::default()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let cache = cache.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    cache
                        .lock()
                        .unwrap()
                        .begin(&[PathBuf::from("same")], Instant::now())
                        .is_some()
                })
            })
            .collect();
        assert_eq!(
            threads
                .into_iter()
                .map(|t| t.join().unwrap() as usize)
                .sum::<usize>(),
            1
        );
    }
    #[test]
    fn older_completions_cannot_replace_newer_results() {
        let now = Instant::now();
        let path = PathBuf::from("fixture");
        let mut cache = Cache::default();
        let old = cache.begin(&[path.clone()], now).unwrap();
        cache.finish_missing(old, true, now);
        let new = cache.begin(&[path.clone()], now).unwrap();
        assert!(cache.publish(new, completed(&path, 200, now), now));
        cache.finish_missing(new, false, now);
        assert!(!cache.publish(old, completed(&path, 1, now), now));
        assert_eq!(cache.cell(&path, now), SizeCell::Exact(200));
    }
    #[test]
    fn manual_child_recalculation_invalidates_an_older_parent_snapshot() {
        let now = Instant::now();
        let root = PathBuf::from("root");
        let child = root.join("child");
        let mut cache = Cache::default();
        let first = cache.begin(&[root.clone()], now).unwrap();
        cache.publish(first, completed(&root, 100, now), now);
        cache.finish_missing(first, false, now);
        assert_eq!(cache.cell(&root, now), SizeCell::Exact(100));
        let next = cache.begin(&[child.clone()], now).unwrap();
        assert_eq!(cache.cell(&root, now), SizeCell::Stale);
        cache.publish(next, completed(&child, 120, now), now);
        cache.finish_missing(next, false, now);
        assert_eq!(cache.cell(&child, now), SizeCell::Exact(120));
        assert_eq!(cache.cell(&root, now), SizeCell::Stale);
    }
    #[test]
    fn invalidation_history_overflow_fails_closed_for_inflight_results() {
        let now = Instant::now();
        let root = PathBuf::from("root");
        let mut cache = Cache::default();
        let ticket = cache.begin(&[root.clone()], now).unwrap();
        for i in 0..140 {
            cache.invalidate(Some(&PathBuf::from(format!("other/{i}"))));
        }
        cache.publish(ticket, completed(&root, 5, now), now);
        cache.finish_missing(ticket, false, now);
        assert_eq!(cache.cell(&root, now), SizeCell::Stale);
    }
    #[test]
    fn reads_and_invalidation_never_create_automatic_work() {
        let now = Instant::now();
        let mut cache = Cache::default();
        for _ in 0..100 {
            assert_eq!(
                cache.cell(Path::new("never-scanned"), now),
                SizeCell::Unknown
            );
        }
        cache.invalidate(Some(Path::new("never-scanned")));
        cache.expire(now + MAX_AGE);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.generation, 0);
    }
    #[test]
    fn retained_children_survive_deeper_result_floods_without_navigation_work() {
        let now = Instant::now();
        let root = PathBuf::from("root");
        let first = root.join("first");
        let second = root.join("second");
        let mut cache = Cache::default();
        let ticket = cache.begin(&[root.clone()], now).unwrap();
        let mut inbox = Inbox::new(&[root.clone()]);
        inbox.push(completed(&first, 1, now));
        for i in 0..1000 {
            inbox.push(completed(&first.join(format!("deep/{i}")), 1, now));
        }
        inbox.push(completed(&second, 2, now));
        inbox.push(completed(&root, 3, now));
        for result in inbox.drain() {
            cache.publish(ticket, result, now);
        }
        for i in 0..1000 {
            cache.publish(
                ticket,
                completed(&second.join(format!("deep/{i}")), 1, now),
                now,
            );
        }
        cache.finish_missing(ticket, false, now);
        assert!(cache.len() <= CACHE_CAPACITY);
        assert_eq!(cache.cell(&first, now), SizeCell::Exact(1));
        assert_eq!(cache.cell(&second, now), SizeCell::Exact(2));
        assert_eq!(cache.cell(&root, now), SizeCell::Exact(3));
        // Navigation only reads: it cannot increment a generation or reserve work.
        for _ in 0..10 {
            cache.metric_cell(&first, Metric::Files, now);
        }
        assert_eq!(cache.generation, ticket.generation);
        assert!(cache.active.is_none());
        cache.invalidate(Some(&first));
        assert_eq!(cache.cell(&root, now), SizeCell::Stale);
        assert_eq!(cache.cell(&first, now), SizeCell::Stale);
        assert_eq!(cache.cell(&second, now), SizeCell::Exact(2));
    }
    #[test]
    fn metric_counts_share_partial_calculating_and_aged_snapshot_states() {
        let now = Instant::now();
        let root = PathBuf::from("root");
        let mut cache = Cache::default();
        assert_eq!(
            cache.metric_cell(&root, Metric::Folders, now),
            SizeCell::Unknown
        );
        let ticket = cache.begin(&[root.clone()], now).unwrap();
        assert_eq!(
            cache.metric_cell(&root, Metric::Files, now),
            SizeCell::Calculating
        );
        let mut result = completed(&root, 4, now);
        result.report.files = 8;
        result.report.folders = 3;
        result.report.skipped = 1;
        result.state = State::Partial;
        cache.publish(ticket, result, now);
        cache.finish_missing(ticket, false, now);
        assert_eq!(
            cache.metric_cell(&root, Metric::Files, now),
            SizeCell::Partial(8)
        );
        assert_eq!(
            cache.metric_cell(&root, Metric::Folders, now + MAX_AGE),
            SizeCell::Partial(3)
        );
        cache.invalidate(Some(&root));
        assert_eq!(
            cache.metric_cell(&root, Metric::Files, now),
            SizeCell::Stale
        );
    }
    #[test]
    fn bounded_cache_and_mailbox_keep_explicit_roots() {
        let now = Instant::now();
        let root = PathBuf::from("root");
        let mut inbox = Inbox::new(&[root.clone()]);
        inbox.push(completed(&root, 10, now));
        for i in 0..1000 {
            inbox.push(completed(&root.join(i.to_string()), i, now));
        }
        let entries = inbox.drain();
        assert_eq!(entries.len(), CACHE_CAPACITY);
        assert!(entries.iter().any(|e| e.path == root));
        let mut cache = Cache::new(MAX_REQUEST_PATHS, MAX_AGE);
        for i in 0..(MAX_REQUEST_PATHS + 5) {
            let path = root.join(i.to_string());
            let ticket = cache.begin(&[path.clone()], now).unwrap();
            cache.publish(ticket, completed(&path, i as u64, now), now);
            cache.finish_missing(ticket, false, now);
        }
        assert_eq!(cache.len(), MAX_REQUEST_PATHS);
        assert!(cache
            .begin(&vec![root; MAX_REQUEST_PATHS + 1], now)
            .is_none());
    }
    #[test]
    fn partial_failed_and_cancelled_results_are_never_exact() {
        let now = Instant::now();
        let path = PathBuf::from("root");
        let mut cache = Cache::default();
        for (report, expected) in [
            (
                SizeReport {
                    bytes: 5,
                    files: 1,
                    skipped: 1,
                    done: true,
                    ..Default::default()
                },
                SizeCell::Partial(5),
            ),
            (
                SizeReport {
                    errors: 1,
                    done: true,
                    ..Default::default()
                },
                SizeCell::Failed,
            ),
            (
                SizeReport {
                    bytes: 5,
                    cancelled: true,
                    done: true,
                    ..Default::default()
                },
                SizeCell::Cancelled,
            ),
        ] {
            let ticket = cache.begin(&[path.clone()], now).unwrap();
            let mut stats = DirectoryStats::new(path.clone(), report);
            stats.observed_at = now;
            cache.publish(ticket, stats, now);
            cache.finish_missing(ticket, false, now);
            assert_eq!(cache.cell(&path, now), expected);
            assert!(cache.exact_bytes(&path, now).is_none());
        }
    }
    #[cfg(windows)]
    #[test]
    fn windows_keys_preserve_unusual_names_and_normalize_prefixes() {
        use std::os::windows::ffi::OsStringExt;
        assert_eq!(
            key(Path::new(r"C:\A\.\B\..\Child")),
            key(Path::new(r"\\?\c:\A\Child"))
        );
        assert_eq!(
            key(Path::new(r"\\?\UNC\server\share\A")),
            key(Path::new(r"\\server\share\A"))
        );
        assert_ne!(key(Path::new(r"C:\case\a")), key(Path::new(r"C:\case\A")));
        let unusual = PathBuf::from(std::ffi::OsString::from_wide(&[
            b'C' as u16,
            58,
            92,
            0xd800,
        ]));
        assert_eq!(key(&unusual).file_name(), unusual.file_name());
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    fn root() -> PathBuf {
        #[cfg(windows)] { PathBuf::from(r"C:\FileOP-fixture\root") }
        #[cfg(not(windows))] { PathBuf::from("/FileOP-fixture/root") }
    }
    fn scanned(path: PathBuf, now: Instant) -> Cache {
        let mut cache = Cache::default();
        let ticket = cache.begin(&[path.clone()], now).unwrap();
        let mut stats = DirectoryStats::new(path, SizeReport { bytes: 12345, files: 7, folders: 3,
            done: true, ..Default::default() });
        stats.observed_at = now;
        cache.publish(ticket, stats, now);
        cache.finish_missing(ticket, false, now);
        cache
    }
    #[test]
    fn restart_keeps_all_metrics_but_never_claims_freshness_or_queues_io() {
        let now = Instant::now(); let wall = SystemTime::now(); let path = root();
        let cache = scanned(path.clone(), now);
        let disk = cache.disk_snapshot(SystemTime::now());
        let json = serde_json::to_vec(&disk).unwrap();
        let mut restored = Cache::default();
        restored.restore_disk(serde_json::from_slice(&json).unwrap(), now, wall + Duration::from_secs(1));
        for (metric, value) in [(Metric::Bytes, 12345), (Metric::Files, 7), (Metric::Folders, 3)] {
            assert_eq!(restored.metric_cell(&path, metric, now), SizeCell::Cached(value));
        }
        assert_eq!(restored.len(), 1);
        assert!(!restored.automatic_candidate(&path, now)); // not visible; reads never enqueue
        restored.set_visible([path.clone()]);
        assert!(!restored.automatic_candidate(&path, now)); // recent snapshots get a refresh interval
        assert!(restored.automatic_candidate(&path, now + REFRESH_AGE));
    }
    #[test]
    fn late_disk_load_cannot_resurrect_mutated_or_replace_live_results() {
        let now = Instant::now(); let path = root();
        let disk = scanned(path.clone(), now).disk_snapshot(SystemTime::now());
        let mut restored = Cache::default();
        restored.invalidate(Some(&path.join("changed.txt")));
        restored.restore_disk(disk.clone(), now, SystemTime::now());
        assert_eq!(restored.cell(&path, now), SizeCell::Stale);
        let mut live = scanned(path.clone(), now);
        live.restore_disk(disk, now, SystemTime::now());
        assert_eq!(live.cell(&path, now), SizeCell::Exact(12345));
        let mut empty = Cache::default();
        for _ in 0..150 { empty.invalidate(Some(&root().join("other"))); }
        empty.restore_disk(scanned(path.clone(), now).disk_snapshot(SystemTime::now()), now, SystemTime::now());
        assert_eq!(empty.cell(&path, now), SizeCell::Stale);
    }
    #[test]
    fn persisted_partial_invalidated_future_expired_and_unknown_versions_are_safe() {
        let now = Instant::now(); let wall = SystemTime::now(); let path = root();
        let disk = scanned(path.clone(), now).disk_snapshot(SystemTime::now());
        for variant in 0..5 {
            let mut copy = disk.clone();
            match variant {
                0 => copy.entries[0].report.skipped = 1,
                1 => copy.entries[0].invalidated = true,
                2 => copy.entries[0].calculated_at = wall + Duration::from_secs(3600),
                3 => copy.entries[0].calculated_at = wall - DISK_MAX_AGE - Duration::from_secs(1),
                _ => copy.version = 2,
            }
            let mut restored = Cache::default();
            restored.restore_disk(copy, now, SystemTime::now());
            assert_eq!(restored.cell(&path, now), match variant {
                0 => SizeCell::Partial(12345), 1 => SizeCell::Stale, _ => SizeCell::Unknown });
        }
    }
    #[test]
    fn navigating_away_during_refresh_preserves_old_values_and_invalidation() {
        let now = Instant::now(); let path = root();
        for mutated in [false, true] {
            let mut cache = scanned(path.clone(), now);
            cache.set_visible([path.clone()]);
            let later = now + REFRESH_AGE;
            let ticket = cache.begin_automatic(&[path.clone()], later).unwrap();
            assert_eq!(cache.disk_snapshot(SystemTime::now()).entries.len(), 1);
            if mutated { cache.invalidate(Some(&path.join("changed"))); }
            cache.finish_missing(ticket, true, later);
            cache.retry_cancelled_scan(ticket);
            assert_eq!(cache.cell(&path, later), if mutated { SizeCell::Stale } else { SizeCell::Cached(12345) });
            assert_eq!(cache.entry(&path).unwrap().stats.report.files, 7);
        }
    }
    #[test]
    fn failed_refresh_keeps_cached_values_without_repeated_scanning() {
        let now = Instant::now(); let path = root(); let mut cache = scanned(path.clone(), now);
        cache.set_visible([path.clone()]);
        let later = now + REFRESH_AGE;
        let ticket = cache.begin_automatic(&[path.clone()], later).unwrap();
        cache.finish_missing(ticket, false, later); // failed worker
        assert_eq!(cache.cell(&path, later), SizeCell::Cached(12345));
        assert!(!cache.automatic_candidate(&path, later + Duration::from_secs(5)));
        assert_eq!(cache.next_expiry(later), Some(REFRESH_AGE));
        cache.expire(later + REFRESH_AGE);
        assert!(cache.automatic_candidate(&path, later + REFRESH_AGE));
    }
    #[test]
    fn refresh_is_bounded_and_does_not_replenish_admission_on_each_tick() {
        let now = Instant::now(); let path = root(); let mut cache = scanned(path.clone(), now);
        cache.set_visible([path.clone()]);
        assert!(!cache.automatic_candidate(&path, now + MAX_AGE));
        assert!(cache.expire(now + REFRESH_AGE));
        let ticket = cache.begin_automatic(&[path.clone()], now + REFRESH_AGE).unwrap();
        assert!(cache.begin_automatic(&[path.clone()], now + REFRESH_AGE).is_none());
        let mut stats = DirectoryStats::new(path.clone(), SizeReport { bytes: 99, done: true, ..Default::default() });
        stats.observed_at = now + REFRESH_AGE;
        cache.publish(ticket, stats, now + REFRESH_AGE);
        cache.finish_missing(ticket, false, now + REFRESH_AGE);
        assert!(!cache.automatic_candidate(&path, now + REFRESH_AGE + MAX_AGE));
    }
    #[cfg(windows)]
    #[test]
    fn cached_unc_and_non_unicode_paths_round_trip_without_filesystem_probes() {
        use std::os::windows::ffi::OsStringExt;
        let now = Instant::now();
        for path in [PathBuf::from(r"\\fileop-offline-invalid\share\folder"),
            PathBuf::from(std::ffi::OsString::from_wide(&[67, 58, 92, 0xd800]))] {
            let disk = scanned(path.clone(), now).disk_snapshot(SystemTime::now());
            let mut restored = Cache::default();
            restored.restore_disk(disk, now, SystemTime::now());
            assert_eq!(restored.cell(&path, now), SizeCell::Cached(12345));
        }
    }
}
