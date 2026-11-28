//! Admission for restored listings and watcher setup across all windows.
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use atlas_model::app_state::AppState;
use atlas_model::layout_tree::TileId;
use atlas_model::location::Location;
use tokio::sync::oneshot;

use crate::app::App;

const RESTORE_IO_LIMIT: usize = 4;
static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    running: 0,
    waiting: VecDeque::new(),
});

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Priority {
    Visible,
    Focused,
    Selected,
}

struct Waiter {
    priority: Priority,
    cancelled: Arc<AtomicBool>,
    ready: oneshot::Sender<Slot>,
}

struct Queue {
    running: usize,
    waiting: VecDeque<Waiter>,
}

impl Queue {
    fn admit(&mut self) {
        while self.running < RESTORE_IO_LIMIT {
            // Take the first waiter in the highest priority class.
            let Some(index) = self
                .waiting
                .iter()
                .enumerate()
                .max_by_key(|(index, waiter)| (waiter.priority, std::cmp::Reverse(*index)))
                .map(|(index, _)| index)
            else {
                break;
            };
            let waiter = self.waiting.remove(index).expect("queue index exists");
            match waiter.ready.send(Slot { admitted: true }) {
                Ok(()) => self.running += 1,
                Err(mut slot) => {
                    // The receiver closed. No slot was charged and Drop
                    // must not attempt to lock this queue recursively.
                    slot.admitted = false;
                }
            }
        }
    }
}

fn queue() -> MutexGuard<'static, Queue> {
    QUEUE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) struct Slot {
    admitted: bool,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.admitted {
            let mut queue = queue();
            queue.running -= 1;
            queue.admit();
        }
    }
}

struct Request(Arc<AtomicBool>);

impl Request {
    fn enqueue(priority: Priority) -> (Self, Admission) {
        let (ready, wait) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut queue = queue();
        let waiter = Waiter {
            priority,
            cancelled: cancelled.clone(),
            ready,
        };
        if priority == Priority::Selected {
            queue.waiting.push_front(waiter);
        } else {
            queue.waiting.push_back(waiter);
        }
        queue.admit();
        (Self(cancelled.clone()), Admission { wait, cancelled })
    }

    fn promote(&self) {
        let mut queue = queue();
        if let Some(index) = queue
            .waiting
            .iter()
            .position(|waiter| Arc::ptr_eq(&waiter.cancelled, &self.0))
        {
            let mut waiter = queue.waiting.remove(index).expect("queue index exists");
            waiter.priority = Priority::Selected;
            queue.waiting.push_front(waiter);
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
        queue()
            .waiting
            .retain(|waiter| !Arc::ptr_eq(&waiter.cancelled, &self.0));
    }
}

pub(super) struct Admission {
    wait: oneshot::Receiver<Slot>,
    cancelled: Arc<AtomicBool>,
}

impl Admission {
    pub(super) async fn wait(self) -> Option<Slot> {
        let slot = self.wait.await.ok()?;
        (!self.cancelled.load(Ordering::Acquire)).then_some(slot)
    }
}

struct Listing {
    request: Request,
    operation: u64,
    explicit_path: bool,
}

#[derive(Default)]
pub(in crate::app) struct RestoredTabs {
    pending: HashMap<TileId, bool>,
    loading: HashMap<TileId, Listing>,
}

impl RestoredTabs {
    pub(in crate::app) fn new(state: &AppState, explicit: Option<TileId>) -> Self {
        let pending = state
            .pane_tree
            .tiles
            .iter()
            .filter_map(|(id, tile)| {
                let atlas_model::layout_tree::Tile::Pane(tab) = tile else {
                    return None;
                };
                matches!(
                    tab.location,
                    Location::Directory(_) | Location::Archive { .. }
                )
                .then_some((*id, explicit == Some(*id)))
            })
            .collect();
        Self {
            pending,
            loading: HashMap::new(),
        }
    }

    pub(in crate::app) fn defer_directory(&mut self, tab: TileId) {
        self.pending.insert(tab, false);
    }

    pub(in crate::app) fn is_pending(&self, tab: TileId) -> bool {
        self.pending.contains_key(&tab)
    }

    pub(in crate::app) fn has_restore(&self, tab: TileId) -> bool {
        self.is_pending(tab) || self.loading.contains_key(&tab)
    }

    pub(in crate::app) fn cancel(&mut self, tab: TileId) -> bool {
        self.pending.remove(&tab);
        self.loading.remove(&tab).is_some()
    }

    pub(in crate::app) fn reschedule(&mut self, tab: TileId) -> bool {
        if let Some(listing) = self.loading.remove(&tab) {
            self.pending.insert(tab, listing.explicit_path);
        }
        self.is_pending(tab)
    }

    pub(in crate::app) fn finish(&mut self, tab: TileId, operation: u64) -> bool {
        if self
            .loading
            .get(&tab)
            .is_some_and(|listing| listing.operation == operation)
        {
            self.loading.remove(&tab);
            true
        } else {
            false
        }
    }
}

impl App {
    pub(in crate::app) fn cancel_restored_listing(&mut self, tab: TileId) {
        if self.restored_tabs.cancel(tab) {
            self.watchers.remove(&tab);
        }
    }

    pub(crate) fn load_visible_restored_tabs(&mut self) {
        if self.waiting_for_moved_tab || self.restored_tabs.pending.is_empty() {
            return;
        }
        let focused = self.state.focused_tile;
        let visible = self.state.pane_tree.visible_panes();
        if visible.contains(&focused) {
            self.enqueue_restored_tab(focused, Priority::Focused);
        }
        for tab in visible.into_iter().filter(|tab| *tab != focused) {
            self.enqueue_restored_tab(tab, Priority::Visible);
        }
    }

    pub(in crate::app) fn prioritize_restored_tab(&mut self, tab: TileId) {
        if let Some(listing) = self.restored_tabs.loading.get(&tab) {
            listing.request.promote();
        } else {
            self.enqueue_restored_tab(tab, Priority::Selected);
        }
    }

    fn enqueue_restored_tab(&mut self, tab: TileId, priority: Priority) {
        if self.tab_closing.contains_key(&tab) || self.pane_closing.contains_key(&tab) {
            return;
        }
        let Some(explicit_path) = self.restored_tabs.pending.remove(&tab) else {
            return;
        };
        let Some(data) = self.tab_data_for(tab) else {
            return;
        };
        let (location, col, dir) = (
            data.location.clone(),
            data.sort_col.clone(),
            data.sort_dir.clone(),
        );
        let (request, admission) = Request::enqueue(priority);
        let operation = match location {
            Location::Directory(path) => {
                self.list_directory(tab, path, col, dir, Some(admission), explicit_path)
            }
            Location::Archive { archive, inner } => {
                Some(self.list_archive(tab, archive, inner, col, dir, Some(admission)))
            }
            _ => None,
        };
        if let Some(operation) = operation {
            self.restored_tabs.loading.insert(
                tab,
                Listing {
                    request,
                    operation,
                    explicit_path,
                },
            );
            self.dirty_panes.insert(tab);
        }
    }
}
