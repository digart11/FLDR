//! Selection metadata and explicit size requests. Render only reads snapshots.
use super::App;
use atlas_model::{
    app_state::BgResult,
    file_entry::{FileEntry, FileKind},
    folder_size::SizeReport,
    location::Location,
    pane::TabId,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::Ordering, Arc, Mutex},
    time::Instant,
};
mod size_job;
mod automatic;
use size_job::{SizeJob, Work};

#[derive(Default)]
pub(crate) struct ContextInfo {
    metadata: HashMap<TabId, MetadataState>,
    pub shortcuts: atlas_model::shortcut_targets::Cache,
    pub stats: atlas_model::folder_stats::Cache,
    pub stats_expiry_at: Option<Instant>,
    pub stats_expiry_generation: u64,
    pub stats_expiry_task: Option<tokio::task::JoinHandle<()>>,
    pub stats_sort_armed: bool,
    pub stats_persist_armed: bool,
    pub stats_loaded: bool,
    pub stats_writer: Option<flume::Sender<atlas_core::fs::stats_cache::WriteRequest>>,
    pub stats_source: u64,
    pub scan: Option<SizeJob<TabId>>,
    next: u64,
    automatic: automatic::Automatic,
}
#[derive(Default)]
struct MetadataState {
    key: Option<(PathBuf, Option<u64>, Option<std::time::SystemTime>)>,
    generation: u64,
    inflight: Option<u64>,
    requested: bool,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    properties: Vec<(String, String, String)>,
}
impl ContextInfo {
    pub(crate) fn with_writer(writer: flume::Sender<atlas_core::fs::stats_cache::WriteRequest>) -> Self {
        let mut state = Self::default();
        static NEXT_SOURCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        state.stats_source = NEXT_SOURCE.fetch_add(1, Ordering::Relaxed);
        state.stats_writer = Some(writer);
        state
    }
}
impl Drop for ContextInfo {
    fn drop(&mut self) {
        if let Some(task) = &self.stats_expiry_task { task.abort(); }
        for state in self.metadata.values() { state.cancel.store(true, Ordering::Relaxed); }
        if let Some(scan) = &self.scan {
            scan.cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl App {
    pub(crate) fn flush_folder_statistics(&self) {
        let Some(writer) = self.context_info.stats_writer.clone().filter(|_| self.context_info.stats_loaded) else { return; };
        let snapshot = self.context_info.stats.disk_snapshot(std::time::SystemTime::now());
        let source = self.context_info.stats_source;
        self.rt.spawn_blocking(move || {
            if let Err(error) = atlas_core::fs::stats_cache::flush_window(&writer, source, snapshot) {
                tracing::warn!(%error, "statistics shutdown flush not confirmed");
            }
        });
    }
    pub(crate) fn arm_stats_persistence(&mut self) {
        if !self.context_info.stats_loaded || self.context_info.stats_persist_armed { return; }
        self.context_info.stats_persist_armed = true;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let _ = tx.send(BgResult::FolderStatsPersist);
        });
    }

    pub(crate) fn shortcut_preview(&mut self, entry: &FileEntry, native: bool) -> Option<String> {
        if atlas_model::settings::shortcut_alpha(self.state.settings.shortcut_target_opacity) == 0.0 {
            self.context_info.shortcuts.cancel();
            return None;
        }
        let key = atlas_model::shortcut_targets::key_for(entry, native, self.state.settings.shortcut_target_opacity)?;
        let now = Instant::now();
        let value = self.context_info.shortcuts.get(&key, now).map(str::to_owned);
        if let Some((ticket, cancel)) = self.context_info.shortcuts.request(key.clone(), now) {
            let tx = self.tx.clone();
            self.rt.spawn_blocking(move || {
                #[cfg(windows)]
                let target = if cancel.load(Ordering::Relaxed) { None } else { atlas_win_space::metadata::shortcut_target(&key.path) };
                #[cfg(not(windows))]
                let target = { let _ = (&key, &cancel); None };
                let target = if cancel.load(Ordering::Relaxed) { None } else { target };
                let _ = tx.send(BgResult::ShortcutTargetReady { ticket, target });
            });
        }
        value
    }
    pub(crate) fn context_selection(&self, tab: TabId) -> Vec<FileEntry> {
        let mode = self
            .tab_data_for(tab)
            .map(|tab| tab.view_mode.clone())
            .unwrap_or_default();
        self.state
            .tab_states
            .get(&tab)
            .map(|state| state.selected_entries(&mode).into_iter().cloned().collect())
            .unwrap_or_default()
    }
    fn size_scope(&self, tab: TabId) -> Option<(Vec<PathBuf>, u64)> {
        let Location::Directory(directory) = &self.tab_data_for(tab)?.location else {
            return None;
        };
        let paths = size_job::scope_paths(directory.clone(), self.selected_paths_for_pane(tab));
        Some((paths, self.state.tab_states.get(&tab)?.entries_version))
    }
    pub(crate) fn request_folder_size(&mut self, tab: TabId) {
        // Keep the accepted Cancel-on-repeat contract, and never overlap workers.
        if !self.context_info.automatic.running && self.context_info.scan.as_ref().is_some_and(|job| !job.report.done) {
            size_job::dispatch(&mut self.context_info.scan, &mut self.context_info.next, None);
            if let Some(job) = &self.context_info.scan { self.dirty_panes.insert(job.tab); }
            return;
        }
        let Some((paths, version)) = self.size_scope(tab) else { return; };
        if paths.len() > atlas_model::folder_stats::MAX_REQUEST_PATHS {
            self.toast_error(rust_i18n::t!("folder_stats_request_limit", count = atlas_model::folder_stats::MAX_REQUEST_PATHS).into_owned());
            return;
        }
        let selected = self.context_selection(tab);
        let directories = if selected.is_empty() { paths.clone() } else {
            selected.iter().filter(|entry| entry.kind() == FileKind::Dir).map(FileEntry::path).collect()
        };
        // Freeze the user's scope before waiting for automatic cancellation.
        if size_job::preempt_automatic(&mut self.context_info.scan, self.context_info.automatic.running,
            &mut self.context_info.automatic.manual, (tab, paths.clone(), version, directories.clone())) { return; }
        self.start_manual_folder_size(tab, paths, version, directories);
    }
    fn start_manual_folder_size(&mut self, tab: TabId, paths: Vec<PathBuf>, version: u64, directories: Vec<PathBuf>) {
        for path in &paths {
            if !directories.contains(path) { self.context_info.stats.invalidate(Some(path)); }
        }
        let Some(ticket) = self.context_info.stats.begin(&directories, Instant::now()) else { return; };
        let inbox = Arc::new(Mutex::new(atlas_model::folder_stats::Inbox::new(&directories)));
        let work = size_job::dispatch(&mut self.context_info.scan, &mut self.context_info.next, Some((tab, paths, version)));
        let Some(Work { tab, generation, paths, cancel }) = work else { return; };
        if let Some(job) = &mut self.context_info.scan {
            job.stats_ticket = Some(ticket); job.stats_inbox = Some(inbox.clone());
        }
        self.dirty_panes.extend(self.state.tab_states.keys().copied());
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            atlas_core::fs::folder_size::scan_with_stats(paths, &cancel,
                |path| atlas_core::vault::is_private_path(path) || atlas_core::vault::live::is_private(path),
                |report| { let _ = tx.send(BgResult::FolderSizeUpdate { tab, generation, report }); },
                |stats| inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(stats));
        });
    }
    pub(crate) fn apply_folder_size(&mut self, tab: TabId, generation: u64, report: SizeReport) {
        let Some(job) = self.context_info.scan.as_mut().filter(|job| job.tab == tab && job.generation == generation) else { return; };
        let Some(ticket) = job.stats_ticket else { return; };
        let now = Instant::now();
        if let Some(inbox) = &job.stats_inbox {
            for stats in inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain() {
                self.context_info.stats.publish(ticket, stats, now);
            }
        }
        let done = report.done; let cancelled = report.cancelled;
        if !job.apply(tab, generation, report) { return; }
        // Results now belong to directories, not a persistent selection notice.
        if done {
            job.dismissed = true;
            self.context_info.stats.finish_missing(ticket, cancelled, now);
            if self.context_info.automatic.running {
                self.context_info.automatic.defer_completed(&job.paths, now);
                if cancelled { self.context_info.stats.retry_cancelled_scan(ticket); }
            }
            self.resort_folder_statistics();
        }
        if done { self.arm_stats_persistence(); }
        self.arm_folder_stats_expiry();
        if done {
            self.context_info.automatic.running = false;
            if let Some((tab, paths, version, directories)) = self.context_info.automatic.manual.take() {
                self.start_manual_folder_size(tab, paths, version, directories);
            }
            // Completion (including empty/cancelled jobs) must not depend on a
            // subsequent repaint to restart eligible viewport work.
            if self.state.settings.automatic_folder_statistics { self.arm_automatic_statistics(); }
        }
    }
    pub(crate) fn arm_folder_stats_expiry(&mut self) {
        let now = Instant::now();
        let Some(delay) = self.context_info.stats.next_expiry(now) else { return; };
        let deadline = now + delay + std::time::Duration::from_millis(250);
        if self.context_info.stats_expiry_at.is_some_and(|at| at <= deadline) { return; }
        if let Some(task) = self.context_info.stats_expiry_task.take() { task.abort(); }
        self.context_info.stats_expiry_at = Some(deadline);
        self.context_info.stats_expiry_generation += 1;
        let generation = self.context_info.stats_expiry_generation;
        let tx = self.tx.clone();
        self.context_info.stats_expiry_task = Some(self.rt.spawn(async move {
            // Coalesce nearby subtree completion deadlines into a single wake.
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
            let _ = tx.send(BgResult::FolderStatsExpire(generation));
        }));
    }
    pub(crate) fn sort_folder_statistics_entries(&self, entries: &mut [FileEntry], col: &atlas_model::file_entry::SortColumn,
        dir: &atlas_model::file_entry::SortDir, order: atlas_model::pane::FolderOrder) {
        if !matches!(col, atlas_model::file_entry::SortColumn::Size | atlas_model::file_entry::SortColumn::Folders | atlas_model::file_entry::SortColumn::Files) { return; }
        let now = Instant::now();
        let metric = match col { atlas_model::file_entry::SortColumn::Folders => atlas_model::folder_stats::Metric::Folders,
            atlas_model::file_entry::SortColumn::Files => atlas_model::folder_stats::Metric::Files, _ => atlas_model::folder_stats::Metric::Bytes };
        let values: HashMap<_, _> = entries.iter().filter(|entry| entry.kind() == FileKind::Dir)
            .map(|entry| (entry.path(), match self.context_info.stats.metric_cell(&entry.path(), metric, now) { atlas_model::folder_stats::SizeCell::Exact(n) | atlas_model::folder_stats::SizeCell::Cached(n) => Some(n), _ => None })).collect();
        if values.is_empty() { return; }
        atlas_core::fs::list::sort_entries_with_sizes(entries, col, dir, order, |entry| {
            if entry.kind() == FileKind::Dir { values.get(&entry.path()).copied().flatten() } else if *col == atlas_model::file_entry::SortColumn::Size { entry.size() } else { None }
        });
    }
    pub(crate) fn resort_folder_statistics(&mut self) {
        // New statistics must not move row indices underneath a marquee,
        // native drag, inline rename or column interaction. Retry in memory only.
        if self.rubber.is_some() || self.details_drag_press.is_some() || self.rename_edit.is_some()
            || self.active_external_drag.is_some() || self.state.dragging_pane.is_some()
            || self.col_resize.is_some() || self.col_dragging.is_some() {
            if !self.context_info.stats_sort_armed {
                self.context_info.stats_sort_armed = true;
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    let _ = tx.send(BgResult::FolderStatsResort);
                });
            }
            return;
        }
        let panes: Vec<_> = self.state.tab_states.keys().copied().filter_map(|pane| {
            let tab = self.tab_data_for(pane)?;
            (tab.view_mode == atlas_model::settings::ViewMode::Details && matches!(tab.sort_col, atlas_model::file_entry::SortColumn::Size | atlas_model::file_entry::SortColumn::Folders | atlas_model::file_entry::SortColumn::Files))
                .then_some((pane, tab.sort_col.clone(), tab.sort_dir.clone(), tab.folder_order))
        }).collect();
        for (pane, col, dir, order) in panes {
            let Some(state) = self.state.tab_states.get(&pane) else { continue; };
            let mut entries = state.entries.to_vec();
            self.sort_folder_statistics_entries(&mut entries, &col, &dir, order);
            if entries.iter().map(FileEntry::path).eq(state.entries.iter().map(FileEntry::path)) { continue; }
            self.mutate_tab(pane, |state| state.set_entries(entries.into()));
            self.file_filter_projections.remove(&pane);
            self.dirty_panes.insert(pane);
        }
    }
    pub(crate) fn invalidate_folder_stats_paths(&mut self, paths: &[PathBuf]) {
        self.arm_stats_persistence();
        let populated = self.context_info.stats.len() > 0;
        for path in paths { self.context_info.stats.invalidate(Some(path)); }
        if !populated {
            if self.state.settings.automatic_folder_statistics { self.arm_automatic_statistics(); }
            return;
        }
        self.resort_folder_statistics();
        self.dirty_panes.extend(self.state.tab_states.keys().copied());
        if self.state.settings.automatic_folder_statistics { self.arm_automatic_statistics(); }
    }
    pub(crate) fn invalidate_folder_stats(&mut self, path: Option<&std::path::Path>) {
        self.arm_stats_persistence();
        let populated = self.context_info.stats.len() > 0;
        self.context_info.stats.invalidate(path);
        if !populated {
            if self.state.settings.automatic_folder_statistics { self.arm_automatic_statistics(); }
            return;
        }
        self.resort_folder_statistics();
        self.dirty_panes.extend(self.state.tab_states.keys().copied());
        if self.state.settings.automatic_folder_statistics { self.arm_automatic_statistics(); }
    }
    /// The working bottom pill remains unchanged; only active manual progress is
    /// exposed here. Completed values live in directory cells across navigation.
    pub(crate) fn manual_folder_scan_running(&self) -> bool {
        !self.context_info.automatic.running && self.context_info.scan.as_ref().is_some_and(|job| !job.report.done)
    }
    pub(crate) fn folder_size_text(&self, tab: TabId) -> Option<String> {
        if self.context_info.automatic.running { return None; }
        let job = self.context_info.scan.as_ref().filter(|job| job.tab == tab && !job.report.done)?;
        Some(rust_i18n::t!(job.message_key(), size = atlas_core::util::display::format_size(job.report.bytes),
            files = job.report.files, errors = job.report.errors, skipped = job.report.skipped).into_owned())
    }
    /// At most one native metadata request per pane. A newer selection replaces
    /// the desired key; an obsolete worker must finish before another is queued.
    pub(crate) fn sync_context_metadata(&mut self, tab: TabId, selected: &[FileEntry]) {
        self.context_info
            .metadata
            .retain(|pane, _| self.state.tab_states.contains_key(pane));
        let native = self
            .tab_data_for(tab)
            .is_some_and(|tab| matches!(tab.location, Location::Directory(_)));
        let key = (native
            && selected.len() == 1
            && matches!(selected[0].kind(), FileKind::File)
            && !selected[0].is_cloud_placeholder()
            && !selected[0].is_symlink()
            && !atlas_core::vault::live::is_private(&selected[0].path()))
        .then(|| (selected[0].path(), selected[0].size(), selected[0].modified));
        let state = self.context_info.metadata.entry(tab).or_default();
        if state.key != key {
            state.cancel.store(true, Ordering::Relaxed);
            state.key = key;
            state.generation += 1;
            state.requested = false;
            state.properties.clear();
        }
        if state.requested || state.inflight.is_some() {
            return;
        }
        let Some((path, _, _)) = state.key.clone() else {
            return;
        };
        state.requested = true;
        let generation = state.generation;
        state.inflight = Some(generation);
        let tx = self.tx.clone();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        state.cancel = cancel.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            let properties = tokio::task::spawn_blocking(move || {
                if cancel.load(Ordering::Relaxed) { return Vec::new(); }
                #[cfg(windows)]
                { atlas_win_space::metadata::named_properties(&path) }
                #[cfg(not(windows))]
                { let _ = path; Vec::new() }
            }).await.unwrap_or_default();
            let _ = tx.send(BgResult::ContextMetadataReady { tab, generation, properties });
        });
    }

    pub(crate) fn apply_context_metadata(
        &mut self,
        tab: TabId,
        generation: u64,
        properties: Vec<(String, String, String)>,
    ) {
        if let Some(state) = self.context_info.metadata.get_mut(&tab) {
            if state.inflight == Some(generation) {
                state.inflight = None;
            }
            if state.generation == generation {
                state.properties = properties;
            }
        }
        // An obsolete request releases the slot for the latest desired file,
        // even when a retained inspector decides it does not need repainting.
        if self.state.tab_states.contains_key(&tab) {
            let selected = self.context_selection(tab);
            self.sync_context_metadata(tab, &selected);
        }
    }
    pub(crate) fn context_properties(&self, tab: TabId, path: &std::path::Path) -> &[(String, String, String)] {
        self.context_info.metadata.get(&tab).filter(|state| state.key.as_ref().is_some_and(|(p, _, _)| p == path))
            .map_or(&[], |state| state.properties.as_slice())
    }
    pub(crate) fn context_metadata_stamp(&self, tab: TabId) -> u64 {
        self.context_info.metadata.get(&tab).map_or(0, |s| s.generation.wrapping_mul(2) + u64::from(s.requested && s.inflight.is_none()))
    }
    pub(crate) fn context_media_details(&self, tab: TabId, selected: &[atlas_model::file_entry::FileEntry]) -> Option<(String, String)> {
        let [item] = selected else { return None; };
        if !matches!(item.kind(), FileKind::File) { return None; }
        let state = self.context_info.metadata.get(&tab)?;
        if !crate::ui::context_metadata::selection_key_matches(state.key.as_ref(), &item.path(), item.size(), item.modified) { return None; }
        let properties = &state.properties;
        let compact = crate::ui::context_metadata::fields_at_level(item.extension().as_deref().unwrap_or(""), properties, crate::ui::context_metadata::level_for(&self.state.settings, item.extension().as_deref().unwrap_or(""))).join(" \u{b7} ");
        if compact.is_empty() { return None; }
        Some((compact, self.context_details(tab, selected)?.1))
    }

    pub(crate) fn context_details(
        &self,
        tab: TabId,
        selected: &[FileEntry],
    ) -> Option<(String, String)> {
        let item = selected.first()?;
        if selected.len() > 1 {
            let extension = item.extension();
            let shared = extension
                .as_ref()
                .is_some_and(|extension| !extension.is_empty())
                && selected.iter().all(|entry| entry.extension() == extension);
            return Some((
                rust_i18n::t!("info_selection", count = selected.len()).into_owned(),
                if shared {
                    extension
                        .as_ref()
                        .map_or_else(String::new, |value| value.to_uppercase().to_string())
                } else {
                    rust_i18n::t!("info_mixed").into_owned()
                },
            ));
        }
        let mut fields = Vec::new();
        if let Some(bytes) = item
            .size()
            .filter(|_| matches!(item.kind(), FileKind::File))
        {
            fields.push(atlas_core::util::display::format_size(bytes));
        }
        if matches!(item.kind(), FileKind::Dir) {
            if let Some(size) = self.folder_size_text(tab) {
                fields.push(size);
            }
        }
        let properties = self
            .context_info
            .metadata
            .get(&tab)
            .filter(|state| crate::ui::context_metadata::selection_key_matches(state.key.as_ref(), &item.path(), item.size(), item.modified))
            .map(|state| &state.properties);
        let compact = properties.map(|values| crate::ui::context_metadata::fields_at_level(
            item.extension().as_deref().unwrap_or(""), values, crate::ui::context_metadata::level_for(&self.state.settings, item.extension().as_deref().unwrap_or(""))).join(" \u{b7} ")).unwrap_or_default();
        if compact.is_empty() {
            if let Some(extension) = item.extension() { fields.push(extension.to_uppercase().to_string()); }
        }
        let basic = fields.join(" \u{b7} ");
        let line = if compact.is_empty() {
            basic.clone()
        } else {
            format!("{basic} · {compact}")
        };
        let mut expanded = format!("{}\n{basic}", item.name);
        if let Some(bytes) = item.size().filter(|_| item.kind() == FileKind::File) {
            expanded.push_str(&format!("\n{bytes} bytes"));
        }
        if let Some(modified) = item.modified {
            expanded.push_str(&format!("\n{}: {}", rust_i18n::t!("column_modified"), atlas_core::util::display::format_date(modified)));
        }
        if let Some(properties) = properties {
            for (key, name, value) in properties {
                if let Some(value) = crate::ui::context_metadata::display_property(key, value) {
                    expanded.push_str(&format!("\n{name}: {value}"));
                }
            }
        }
        let compact = if line.is_empty() { item.name.to_string() } else { format!("{} \u{b7} {line}", item.name) };
        Some((compact, expanded))
    }
}
