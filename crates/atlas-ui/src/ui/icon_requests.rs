//! Request identity and retry policy for the bounded Shell icon cache.
use std::{collections::HashMap, path::{Path, PathBuf}, time::{Duration,Instant}};
#[derive(Default)]
pub(crate) struct Requests { next: u64, pending: HashMap<PathBuf,u64>, failed: HashMap<PathBuf,Instant> }
impl Requests {
    pub fn begin(&mut self, key: &Path, now: Instant) -> Option<u64> {
        if self.pending.contains_key(key) || self.failed.get(key).is_some_and(|at| now.duration_since(*at)<Duration::from_secs(30)) {return None;}
        self.failed.remove(key);
        self.next=self.next.wrapping_add(1);
        self.pending.insert(key.to_path_buf(),self.next); Some(self.next)
    }
    pub fn complete(&mut self,key:&Path,ticket:u64,success:bool,now:Instant)->bool {
        if self.pending.get(key)!=Some(&ticket) {return false;}
        self.pending.remove(key);
        if !success {
            if self.failed.len() >= 512 {
                if let Some(old) = self.failed.iter().min_by_key(|(_,at)| *at).map(|(key,_)| key.clone()) {self.failed.remove(&old);}
            }
            self.failed.insert(key.to_path_buf(),now);
        }
        true
    }
    pub fn forget(&mut self,key:&Path) {self.pending.remove(key);self.failed.remove(key);}
}
pub(crate) fn folder_key(path: &Path) -> PathBuf {PathBuf::from(format!("__atlas_shell_folder__{}",path.display()))}
