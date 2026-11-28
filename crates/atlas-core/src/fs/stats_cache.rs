//! Local-only snapshot persistence. All callers must use a background worker.
use atlas_model::folder_stats::{DiskCache, CACHE_CAPACITY};
use std::{io::Read, path::{Path, PathBuf}};
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;
pub fn path() -> Option<PathBuf> {
    // No cwd fallback: never put runtime data in the synchronized source tree.
    dirs::data_local_dir().map(|p| p.join("FLDR").join("Cache").join("folder-statistics-v1.json"))
}
pub fn load(path: &Path) -> Option<DiskCache> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES { return None; }
    let cache: DiskCache = serde_json::from_slice(&bytes).ok()?;
    (cache.version == 1 && cache.entries.len() <= CACHE_CAPACITY).then_some(cache)
}
pub fn save(path: &Path, cache: &DiskCache) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(cache)?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_BYTES, "folder statistics snapshot exceeds disk limit");
    crate::util::config::write_atomic(path, &bytes)?;
    Ok(())
}

/// Same snapshot writer, with an optional acknowledgement for orderly exit.
pub struct WriteRequest {
    pub source: u64,
    pub snapshot: DiskCache,
    pub ack: Option<flume::Sender<Result<(), String>>>,
}
impl WriteRequest {
    pub fn snapshot(snapshot: DiskCache) -> Self { Self::for_window(0, snapshot) }
    pub fn for_window(source: u64, snapshot: DiskCache) -> Self { Self { source, snapshot, ack: None } }
}
pub fn writer_loop(path: Option<PathBuf>, rx: flume::Receiver<WriteRequest>) {
    let mut entries: Vec<(u64, atlas_model::folder_stats::DiskEntry)> = Vec::new();
    while let Ok(request) = rx.recv() {
        let mut acks = Vec::new();
        for mut request in std::iter::once(request).chain(rx.try_iter()) {
            if let Some(ack) = request.ack.take() { acks.push(ack); }
            entries.retain(|(source, _)| *source != request.source);
            for entry in request.snapshot.entries.into_iter().rev() {
                if let Some(index) = entries.iter().position(|(_, cached)| cached.path == entry.path) {
                    let cached = &entries[index].1;
                    if cached.calculated_at > entry.calculated_at
                        || cached.calculated_at == entry.calculated_at && cached.invalidated
                    {
                        continue;
                    }
                    entries.remove(index);
                }
                entries.push((request.source, entry));
            }
            let excess = entries.len().saturating_sub(CACHE_CAPACITY);
            entries.drain(..excess);
        }
        let snapshot = DiskCache { version: 1, entries: entries.iter().rev().map(|(_, entry)| entry.clone()).collect() };
        let result = path.as_deref().ok_or_else(|| "No local statistics cache path".to_string())
            .and_then(|path| save(path, &snapshot).map_err(|e| e.to_string()));
        if let Err(error) = &result { tracing::warn!(%error, "folder statistics cache write failed"); }
        for ack in acks { let _ = ack.send(result.clone()); }
    }
}
pub fn flush(writer: &flume::Sender<WriteRequest>, snapshot: DiskCache) -> Result<(), String> {
    flush_window(writer, 0, snapshot)
}

pub fn flush_window(writer: &flume::Sender<WriteRequest>, source: u64, snapshot: DiskCache) -> Result<(), String> {
    let (tx, rx) = flume::bounded(1);
    writer.send_timeout(WriteRequest { source, snapshot, ack: Some(tx) }, std::time::Duration::from_secs(5)).map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(5)).map_err(|e| e.to_string())?
}
