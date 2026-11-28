//! Dispatch and result state for the existing explicit Folder Size command.
//! No traversal or UI layout lives here.
use atlas_model::folder_size::SizeReport;
use std::{path::PathBuf, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Instant};

pub(crate) struct SizeJob<P> {
    pub tab: P,
    pub generation: u64,
    pub paths: Vec<PathBuf>,
    pub version: u64,
    pub report: SizeReport,
    pub cancel: Arc<AtomicBool>,
    pub completed: Option<Instant>,
    pub dismissed: bool,
    pub ui_revision: u64,
    pub stats_ticket: Option<atlas_model::folder_stats::Ticket>,
    pub stats_inbox: Option<Arc<std::sync::Mutex<atlas_model::folder_stats::Inbox>>>,
}
pub(crate) struct Work<P> {
    pub tab: P,
    pub generation: u64,
    pub paths: Vec<PathBuf>,
    pub cancel: Arc<AtomicBool>,
}
/// Queue manual intent until the automatic worker acknowledges cancellation.
/// A blocked filesystem call can delay it, but must never create two traversals.
pub(crate) fn preempt_automatic<P: Copy, R>(scan: &mut Option<SizeJob<P>>, automatic: bool,
    pending: &mut Option<R>, request: R) -> bool {
    if automatic {
        if let Some(job) = scan.as_mut().filter(|job| !job.report.done) {
            *pending = Some(request);
            job.request_cancel();
            return true;
        }
    }
    false
}
pub(crate) fn scope_paths(directory: PathBuf, mut selected: Vec<PathBuf>) -> Vec<PathBuf> {
    if selected.is_empty() { selected.push(directory); }
    selected.sort();
    selected.dedup();
    selected
}
/// Repeating the command cancels the existing job, even from another pane.
/// The calculating state is installed before returning work to the executor.
pub(crate) fn dispatch<P: Copy>(scan: &mut Option<SizeJob<P>>, next: &mut u64,
    scope: Option<(P, Vec<PathBuf>, u64)>) -> Option<Work<P>> {
    if let Some(job) = scan.as_mut().filter(|job| !job.report.done) {
        job.request_cancel();
        return None;
    }
    let (tab, paths, version) = scope?;
    *next += 1;
    let cancel = Arc::new(AtomicBool::new(false));
    *scan = Some(SizeJob { tab, generation: *next, paths: paths.clone(), version,
        report: SizeReport::default(), cancel: cancel.clone(), completed: None, dismissed: false, ui_revision: 1,
        stats_ticket: None, stats_inbox: None });
    Some(Work { tab, generation: *next, paths, cancel })
}
impl<P> SizeJob<P> {
    pub fn request_cancel(&mut self) -> bool {
        if self.report.done { return false; }
        self.cancel.store(true, Ordering::Relaxed);
        self.ui_revision += 1;
        true
    }
}
impl<P: PartialEq> SizeJob<P> {
    pub fn notice_visible(&self, tab: P) -> bool { self.tab == tab && !self.dismissed }
    pub fn dismiss(&mut self) -> bool {
        if !self.report.done { return false; }
        self.dismissed = true;
        self.ui_revision += 1;
        true
    }
    pub fn apply(&mut self, tab: P, generation: u64, report: SizeReport) -> bool {
        if self.tab != tab || self.generation != generation { return false; }
        // A late progress message must not turn a completed job back into a running job.
        if self.report.done && !report.done { return false; }
        if report.done { self.completed = Some(Instant::now()); }
        self.report = report;
        self.ui_revision += 1;
        true
    }
    pub fn message_key(&self) -> &'static str {
        if !self.report.done {
            if self.cancel.load(Ordering::Relaxed) { "size_cancelling" } else { "size_running" }
        } else if self.report.cancelled { "size_cancelled" }
        else if self.report.errors > 0 || self.report.skipped > 0 { "size_partial" }
        else { "size_completed" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_preempts_automatic_without_starting_a_second_worker() {
        let (mut jobs, mut next, mut pending) = (None, 0, None);
        let auto = dispatch(&mut jobs, &mut next, Some((1u64, vec!["auto".into()], 1))).unwrap();
        assert!(preempt_automatic(&mut jobs, true, &mut pending, 2));
        assert!(auto.cancel.load(Ordering::Relaxed));
        assert_eq!(pending, Some(2));
        assert_eq!(next, 1);
        assert!(dispatch(&mut jobs, &mut next, Some((2, vec!["manual".into()], 1))).is_none());
        assert!(jobs.as_mut().unwrap().apply(1, auto.generation, SizeReport { done: true, cancelled: true, ..Default::default() }));
        let manual = dispatch(&mut jobs, &mut next, Some((pending.take().unwrap(), vec!["manual".into()], 1))).unwrap();
        assert_eq!(manual.tab, 2);
        assert_eq!(manual.generation, 2);
        assert!(!preempt_automatic(&mut jobs, false, &mut pending, 1));
        assert!(!manual.cancel.load(Ordering::Relaxed));
    }
    #[test]
    fn queued_manual_scope_survives_navigation_while_cancelling() {
        let (mut jobs, mut next, mut pending) = (None, 0, None);
        dispatch(&mut jobs, &mut next, Some((1u64, vec!["auto".into()], 1))).unwrap();
        let mut selection = vec![PathBuf::from("original/selected-folder")];
        assert!(preempt_automatic(&mut jobs, true, &mut pending, (2u64, selection.clone(), 9)));
        selection[0] = PathBuf::from("new-directory/new-selection");
        let (tab, queued, version) = pending.take().unwrap();
        assert_eq!((tab, version), (2, 9));
        assert_eq!(queued, [PathBuf::from("original/selected-folder")]);
        assert_ne!(queued, selection);
    }
    #[test]
    fn dispatch_resolves_selection_or_directory_and_installs_calculating_state() {
        let dir = PathBuf::from("fixture-directory");
        assert_eq!(scope_paths(dir.clone(), vec![]), [dir.clone()]);
        let file = dir.join("selected.txt");
        assert_eq!(scope_paths(dir, vec![file.clone(), file.clone()]), [file.clone()]);
        let (mut jobs, mut next) = (None, 0);
        let work = dispatch(&mut jobs, &mut next, Some((7u64, vec![file.clone()], 12))).unwrap();
        let job = jobs.as_ref().unwrap();
        assert_eq!(job.paths, [file.clone()]);
        assert!(job.notice_visible(7));
        assert!(!job.notice_visible(8));
        assert_eq!((work.tab, work.generation, work.paths), (7, 1, vec![file]));
        assert_eq!(job.version, 12);
        assert_eq!(job.message_key(), "size_running");
        assert!(!job.report.done && !job.dismissed);
        assert!(dispatch::<u64>(&mut None, &mut next, None).is_none());
    }
    #[test]
    fn repeat_dispatch_cancels_and_completion_can_be_explicitly_refreshed() {
        let (mut jobs, mut next) = (None, 0);
        let first = dispatch(&mut jobs, &mut next, Some((1u64, vec![PathBuf::from("a")], 1))).unwrap();
        assert!(dispatch(&mut jobs, &mut next, Some((2, vec![PathBuf::from("b")], 2))).is_none());
        assert!(first.cancel.load(Ordering::Relaxed));
        let job = jobs.as_mut().unwrap();
        assert_eq!(job.ui_revision, 2);
        assert!(!job.dismiss());
        assert_eq!(job.message_key(), "size_cancelling");
        assert!(job.apply(1, first.generation, SizeReport { done: true, cancelled: true, bytes: 42, ..Default::default() }));
        assert_eq!(job.message_key(), "size_cancelled");
        assert_eq!(job.ui_revision, 3);
        assert!(!job.request_cancel()); // A late Cancel must never restart work.
        assert!(job.dismiss());
        assert!(!job.notice_visible(1));
        assert_eq!(job.ui_revision, 4);
        let next_work = dispatch(&mut jobs, &mut next, Some((2, vec![PathBuf::from("b")], 2))).unwrap();
        assert_eq!(next_work.generation, first.generation + 1);
        assert!(!jobs.as_ref().unwrap().dismissed);
    }
    #[test]
    fn matching_result_survives_listing_version_changes_and_rejects_stale_workers() {
        let (mut jobs, mut next) = (None, 0);
        let work = dispatch(&mut jobs, &mut next, Some((3u64, vec![PathBuf::from("a")], 9))).unwrap();
        let job = jobs.as_mut().unwrap();
        assert!(!job.apply(4, work.generation, SizeReport { done: true, ..Default::default() }));
        assert!(!job.apply(3, work.generation + 1, SizeReport { done: true, ..Default::default() }));
        assert!(job.apply(3, work.generation, SizeReport { done: true, bytes: 100, errors: 1, ..Default::default() }));
        assert_eq!(job.message_key(), "size_partial");
        assert_eq!((job.version, job.report.bytes), (9, 100));
        assert!(job.completed.is_some());
        assert!(!job.apply(3, work.generation, SizeReport::default()));
        assert!(job.report.done);
    }
    #[test]
    fn dispatch_worker_completion_reaches_the_same_ui_job_state() {
        let dir = std::env::temp_dir().join(format!("FileOP-recovery-dispatch-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("sample.bin");
        std::fs::write(&file, b"1234567").unwrap();
        let (mut jobs, mut next) = (None, 0);
        let work = dispatch(&mut jobs, &mut next, Some((1u64, scope_paths(dir.clone(), vec![file.clone()]), 5))).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            atlas_core::fs::folder_size::scan(work.paths, &work.cancel, |_| false, |report| {
                tx.send((work.tab, work.generation, report)).unwrap();
            });
        });
        assert_eq!(jobs.as_ref().unwrap().message_key(), "size_running");
        worker.join().unwrap();
        for (tab, generation, report) in rx { assert!(jobs.as_mut().unwrap().apply(tab, generation, report)); }
        let job = jobs.unwrap();
        assert_eq!((job.report.bytes, job.report.files), (7, 1));
        assert_eq!(job.message_key(), "size_completed");
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
