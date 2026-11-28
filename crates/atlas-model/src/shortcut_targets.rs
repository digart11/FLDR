//! Bounded positive/negative shortcut cache. No destination I/O or queued work.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub path: PathBuf,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
}
/// Gate every lookup before scheduling work; disabled and virtual rows do no I/O.
pub fn key_for(entry: &crate::file_entry::FileEntry, native: bool, opacity: f32) -> Option<Key> {
    if crate::settings::shortcut_alpha(opacity) == 0.0
        || !native
        || entry.kind() != crate::file_entry::FileKind::File
        || entry.is_cloud_placeholder()
        || entry.is_symlink()
        || !crate::file_presentation::extension(&entry.name)
            .is_some_and(|s| s.eq_ignore_ascii_case("lnk"))
    {
        return None;
    }
    Some(Key {
        path: entry.path(),
        size: entry.size(),
        modified: entry.modified,
    })
}

#[derive(Default)]
pub struct Cache {
    values: HashMap<Key, (Option<String>, Instant)>,
    pending: HashMap<u64, (Key, Arc<AtomicBool>)>,
    next: u64,
}
impl Cache {
    pub fn get(&self, key: &Key, now: Instant) -> Option<&str> {
        self.values
            .get(key)
            .filter(|(_, at)| now.duration_since(*at) < Duration::from_secs(60))
            .and_then(|(value, _)| value.as_deref())
    }
    pub fn request(&mut self, key: Key, now: Instant) -> Option<(u64, Arc<AtomicBool>)> {
        if self
            .values
            .get(&key)
            .is_some_and(|(_, at)| now.duration_since(*at) < Duration::from_secs(60))
        {
            return None;
        }
        self.values.remove(&key);
        if self.pending.len() >= 2 || self.pending.values().any(|(k, _)| k == &key) {
            return None;
        }
        self.next += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending.insert(self.next, (key, cancel.clone()));
        Some((self.next, cancel))
    }
    pub fn complete(&mut self, ticket: u64, value: Option<String>, now: Instant) {
        let Some((key, cancel)) = self.pending.remove(&ticket) else {
            return;
        };
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if self.values.len() >= 1024 {
            if let Some(oldest) = self
                .values
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(key, _)| key.clone())
            {
                self.values.remove(&oldest);
            }
        }
        self.values.insert(key, (value, now));
    }
    pub fn cancel(&mut self) {
        for (_, cancel) in self.pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        // Keep pending slots occupied until native calls finish: toggling settings
        // cannot accumulate blocked OS workers. Cached answers remain reusable.
    }
}
impl Drop for Cache {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(name: &str) -> Key {
        Key {
            path: name.into(),
            size: Some(1),
            modified: None,
        }
    }
    #[test]
    fn caches_failures_and_successes_until_expiry_or_fingerprint_change() {
        let mut c = Cache::default();
        let now = Instant::now();
        let k = key("a.lnk");
        let (ticket, _) = c.request(k.clone(), now).unwrap();
        assert!(c.request(k.clone(), now).is_none());
        c.complete(ticket, Some(r"\\server\share\target".into()), now);
        assert_eq!(c.get(&k, now), Some(r"\\server\share\target"));
        assert!(c.request(k.clone(), now).is_none());
        let mut changed = k.clone();
        changed.size = Some(2);
        assert!(c.request(changed, now).is_some());
        let (ticket, _) = c.request(key("bad.lnk"), now).unwrap();
        c.complete(ticket, None, now);
        assert!(c.request(key("bad.lnk"), now).is_none());
        assert!(c.request(k, now + Duration::from_secs(61)).is_some());
    }
    #[test]
    fn cancellation_retains_worker_bound_and_rejects_stale_completion() {
        let mut c = Cache::default();
        let now = Instant::now();
        let (a, token) = c.request(key("a"), now).unwrap();
        let (b, _) = c.request(key("b"), now).unwrap();
        c.cancel();
        assert!(token.load(Ordering::Relaxed));
        assert!(c.request(key("c"), now).is_none());
        c.complete(a, Some("ignored".into()), now);
        c.complete(b, None, now);
        assert!(c.get(&key("a"), now).is_none());
        assert!(c.request(key("a"), now).is_some());
    }
    #[test]
    fn capacity_is_bounded() {
        let mut c = Cache::default();
        let now = Instant::now();
        for i in 0..1100 {
            let (t, _) = c.request(key(&i.to_string()), now).unwrap();
            c.complete(t, None, now);
        }
        assert_eq!(c.values.len(), 1024);
    }
}
