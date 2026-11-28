//! Viewport-driven orchestration for the shared scanner and cache.
use super::{App, SizeJob, Work};
use atlas_model::{
    app_state::BgResult,
    automatic_stats::{enabled, StorageKind},
    folder_stats,
    location::Location,
    pane::TabId,
    tab::LoadState,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(PartialEq)]
struct ViewportIntent {
    directory: PathBuf,
    span: (Option<usize>, Option<usize>, usize),
    sort: (
        atlas_model::file_entry::SortColumn,
        atlas_model::file_entry::SortDir,
    ),
    filter: String,
    kinds: (bool, bool),
    settings: [bool; 5],
}

#[derive(Default)]
pub(super) struct Automatic {
    visible: HashMap<TabId, (PathBuf, Vec<PathBuf>)>,
    intents: HashMap<TabId, ViewportIntent>,
    storage: atlas_model::automatic_stats::Classifications,
    armed: bool,
    observation: atlas_model::automatic_stats::ObservationWake,
    priority: Option<TabId>,
    batch_wait: atlas_model::automatic_stats::BatchWindow,
    retry_after: atlas_model::automatic_stats::RetryWindow,
    pub running: bool,
    pub manual: Option<(TabId, Vec<PathBuf>, u64, Vec<PathBuf>)>,
}

impl Automatic {
    pub(super) fn defer_completed(&mut self, paths: &[PathBuf], now: Instant) {
        self.retry_after.defer(paths, now);
    }
}

impl App {
    /// Called after a file view has a live, laid-out viewport. Never performs IO.
    pub(crate) fn observe_automatic_statistics(&mut self, tab: TabId, rows: &[usize]) {
        let Some(data) = self.tab_data_for(tab) else {
            return;
        };
        let Location::Directory(directory) = &data.location else {
            return;
        };
        let directory = directory.clone();
        let Some(state) = self.state.tab_states.get(&tab) else {
            return;
        };
        if state.load_state != LoadState::Loaded {
            return;
        }
        let intent = ViewportIntent {
            directory: directory.clone(),
            span: (rows.first().copied(), rows.last().copied(), rows.len()),
            sort: (data.sort_col.clone(), data.sort_dir.clone()),
            filter: state.filter.to_string(),
            kinds: (state.show_files, state.show_folders),
            settings: [
                self.state.settings.automatic_folder_statistics,
                self.state.settings.folder_statistics_ssd,
                self.state.settings.folder_statistics_hdd,
                self.state.settings.folder_statistics_network,
                self.state.settings.show_hidden,
            ],
        };
        let intent_changed = self.context_info.automatic.intents.get(&tab) != Some(&intent);
        self.context_info.automatic.intents.insert(tab, intent);
        let paths = atlas_model::automatic_stats::visible_folders(&directory, &state.entries, rows);
        let priority_changed =
            self.context_info.automatic.priority != Some(self.state.focused_tile);
        let changed = intent_changed
            || self.context_info.automatic.visible.get(&tab)
                != Some(&(directory.clone(), paths.clone()))
            || priority_changed;
        self.context_info.automatic.priority = Some(self.state.focused_tile);
        self.context_info
            .automatic
            .visible
            .insert(tab, (directory, paths));
        self.protect_statistics_viewports(intent_changed || priority_changed);
        let auto = &mut self.context_info.automatic;
        if auto
            .observation
            .update(changed, auto.running, &self.state.settings)
        {
            self.arm_automatic_statistics();
        }
    }

    /// Same live-pane checks and focus priority as dispatch. Protect terminal
    /// visible snapshots across batches before descendant results can evict them.
    fn protect_statistics_viewports(&mut self, reset_budget: bool) {
        let mut panes = self.state.pane_tree.visible_panes();
        panes.sort_by_key(|tab| (*tab != self.state.focused_tile, *tab));
        let mut paths = Vec::new();
        for tab in panes {
            let Some(data) = self.tab_data_for(tab) else {
                continue;
            };
            let Some(state) = self.state.tab_states.get(&tab) else {
                continue;
            };
            let Some((directory, visible)) = self.context_info.automatic.visible.get(&tab) else {
                continue;
            };
            if atlas_model::automatic_stats::viewport_enabled(&data.view_mode)
                && state.load_state == LoadState::Loaded
                && matches!(&data.location, Location::Directory(path) if path == directory)
            {
                paths.extend(visible.iter().cloned());
            }
        }
        if reset_budget {
            self.context_info.stats.set_visible(paths);
        } else {
            self.context_info.stats.update_visible(paths);
        }
    }

    pub(crate) fn arm_automatic_statistics(&mut self) {
        if self.context_info.automatic.armed {
            return;
        }
        self.context_info.automatic.armed = true;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = tx.send(BgResult::AutomaticFolderStats);
        });
    }

    pub(crate) fn invalidate_automatic_statistics_storage(&mut self) {
        self.context_info.automatic.storage.invalidate();
        self.arm_automatic_statistics();
    }
    pub(crate) fn apply_statistics_storage(
        &mut self,
        path: PathBuf,
        kind: StorageKind,
        generation: u64,
    ) {
        self.context_info
            .automatic
            .storage
            .complete(&path, kind, generation, Instant::now());
        self.arm_automatic_statistics();
    }

    pub(crate) fn automatic_statistics_tick(&mut self) {
        self.context_info.automatic.armed = false;
        // Cache loading is local and asynchronous. Let it populate snapshots
        // before treating visible folders as unknown and starting duplicate IO.
        if !self.context_info.stats_loaded { return; }
        let visible_panes = self.state.pane_tree.visible_panes();
        let live: HashMap<_, _> = visible_panes
            .into_iter()
            .filter_map(|tab| {
                let data = self.tab_data_for(tab)?;
                let state = self.state.tab_states.get(&tab)?;
                match &data.location {
                    Location::Directory(dir)
                        if atlas_model::automatic_stats::viewport_enabled(&data.view_mode)
                            && state.load_state == LoadState::Loaded =>
                    {
                        Some((tab, dir.clone()))
                    }
                    _ => None,
                }
            })
            .collect();
        let auto = &mut self.context_info.automatic;
        auto.visible
            .retain(|tab, (dir, _)| live.get(tab) == Some(dir));
        auto.intents.retain(|tab, _| live.contains_key(tab));
        self.protect_statistics_viewports(false);
        let auto = &mut self.context_info.automatic;
        let allowed = self.state.settings.automatic_folder_statistics;
        if auto.running {
            if let Some(job) = &mut self.context_info.scan {
                let wanted = allowed
                    && auto.visible.get(&job.tab).is_some_and(|(_, paths)| {
                        job.paths.iter().all(|path| paths.contains(path))
                    })
                    && job.paths.iter().all(|path| {
                        auto.storage
                            .known(path)
                            .is_some_and(|kind| enabled(&self.state.settings, kind))
                    });
                if !wanted {
                    job.request_cancel();
                }
                if !job.report.done {
                    self.arm_automatic_statistics();
                    return;
                }
            }
        }
        if !allowed
            || self
                .context_info
                .scan
                .as_ref()
                .is_some_and(|job| !job.report.done)
        {
            return;
        }
        let mut panes: Vec<_> = self
            .context_info
            .automatic
            .visible
            .keys()
            .copied()
            .collect();
        panes.sort_by_key(|tab| (*tab != self.state.focused_tile, *tab));
        let now = Instant::now();
        let mut waiting_for_retry = false;
        for tab in panes {
            let paths = self.context_info.automatic.visible[&tab].1.clone();
            let mut candidates = Vec::new();
            let mut batch_kind = None;
            for path in paths {
                // Keep partial/failed results; completed snapshots refresh only after the longer refresh interval.
                if !self.context_info.stats.automatic_candidate(&path, now) {
                    continue;
                }
                if !self.context_info.automatic.retry_after.ready(&path, now) {
                    waiting_for_retry = true;
                    continue;
                }
                #[cfg(windows)]
                if !self.state.settings.folder_statistics_network
                    && path.components().any(|c| matches!(c, std::path::Component::Prefix(p)
                        if matches!(p.kind(), std::path::Prefix::UNC(..) | std::path::Prefix::VerbatimUNC(..)))) {
                    continue;
                }
                let kind = self.context_info.automatic.storage.get(&path, now);
                let Some(kind) = kind else {
                    if let Some(generation) = self.context_info.automatic.storage.begin(&path) {
                        let tx = self.tx.clone();
                        self.rt.spawn_blocking(move || {
                            #[cfg(windows)]
                            let _priority =
                                atlas_win_space::storage::BackgroundStatisticsPriority::enter();
                            #[cfg(windows)]
                            let kind = atlas_win_space::storage::statistics_storage(&path);
                            #[cfg(not(windows))]
                            let kind = StorageKind::Unknown;
                            let _ = tx.send(BgResult::FolderStatsStorage {
                                path,
                                kind,
                                generation,
                            });
                        });
                    }
                    // A slow classification must not starve already classified siblings.
                    continue;
                };
                if !enabled(&self.state.settings, kind) {
                    continue;
                }
                if batch_kind.is_some_and(|first| first != kind) {
                    continue;
                }
                batch_kind = Some(kind);
                candidates.push(path);
                if candidates.len()
                    >= atlas_model::automatic_stats::concurrency(kind)
                        .min(self.context_info.stats.automatic_slots())
                {
                    break;
                }
            }
            if candidates.is_empty() {
                continue;
            }
            let paths = candidates;
            let concurrency = atlas_model::automatic_stats::concurrency(batch_kind.unwrap())
                .min(self.context_info.stats.automatic_slots());
            if !self.context_info.automatic.batch_wait.ready(
                &paths[0],
                paths.len(),
                concurrency,
                self.context_info.automatic.storage.is_pending(),
                now,
            ) {
                self.arm_automatic_statistics();
                return;
            }
            let Some(ticket) = self.context_info.stats.begin_automatic(&paths, now) else {
                return;
            };
            let inbox = Arc::new(Mutex::new(folder_stats::Inbox::new(&paths)));
            let version = self.state.tab_states[&tab].entries_version;
            let Some(Work {
                tab,
                generation,
                paths,
                cancel,
            }) = super::size_job::dispatch(
                &mut self.context_info.scan,
                &mut self.context_info.next,
                Some((tab, paths, version)),
            )
            else {
                return;
            };
            let job: &mut SizeJob<TabId> = self.context_info.scan.as_mut().unwrap();
            job.dismissed = true;
            job.stats_ticket = Some(ticket);
            job.stats_inbox = Some(inbox.clone());
            self.context_info.automatic.running = true;
            let tx = self.tx.clone();
            self.rt.spawn_blocking(move || {
                atlas_core::fs::folder_size::scan_automatic(
                    paths,
                    &cancel,
                    concurrency,
                    |report| {
                        let _ = tx.send(BgResult::FolderSizeUpdate {
                            tab,
                            generation,
                            report,
                        });
                    },
                    |stats| {
                        inbox
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(stats)
                    },
                );
            });
            self.arm_automatic_statistics();
            return;
        }
        if waiting_for_retry {
            self.arm_automatic_statistics();
        }
    }
}
