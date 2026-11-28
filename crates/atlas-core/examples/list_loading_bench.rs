//! Repeatable warm-cache listing benchmark; only --create writes fixtures.
#![allow(unsafe_code)]
use std::{alloc::{GlobalAlloc, Layout, System}, path::Path, sync::atomic::{AtomicU64, Ordering::Relaxed}, time::Instant};
use atlas_model::{file_entry::{FileEntry, SortColumn, SortDir}, pane::FolderOrder};

struct CountingAllocator;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            let live = LIVE.fetch_add(layout.size() as u64, Relaxed) + layout.size() as u64;
            PEAK.fetch_max(live, Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as u64, Relaxed);
        unsafe { System.dealloc(ptr, layout); }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[cfg(windows)]
fn cpu_us() -> u64 {
    #[link(name = "kernel32")]
    extern "system" { fn GetProcessTimes(process: isize, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> i32; }
    let (mut created, mut exited, mut kernel, mut user) = (0, 0, 0, 0);
    assert_ne!(unsafe { GetProcessTimes(-1, &mut created, &mut exited, &mut kernel, &mut user) }, 0);
    (kernel + user) / 10
}
#[cfg(not(windows))]
fn cpu_us() -> u64 { 0 }

fn create(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir(root)?;
    for (workload, count) in [("short", 20_000), ("long_unicode", 12_000)] {
        let path = root.join(workload);
        std::fs::create_dir(&path)?;
        for i in 0..count {
            let n = (i * 7919) % count;
            let name = if workload == "short" { format!("f{n}.txt") }
                else { format!("Project-{n}-résumé-Σ-документ-with-a-long-filename.data") };
            std::fs::write(path.join(name), [7; 3])?;
        }
        for i in 0..64 { std::fs::create_dir(path.join(format!("dir-{i}")))?; }
        for i in 0..8 { std::fs::write(path.join(format!(".hidden-{i}")), [9])?; }
    }
    std::fs::write(root.join("fixture-version"), b"fldr-list-loading-v1")?;
    Ok(())
}

fn checksum(entries: &[FileEntry]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for entry in entries {
        let values = format!("{}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}\n", entry.name,
            entry.path(), entry.kind(), entry.size(), entry.modified, entry.file_id(),
            entry.is_hidden(), entry.is_readonly(), entry.is_cloud_placeholder());
        for byte in values.bytes() { hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3); }
    }
    hash
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3 && args[1] == "--create" { create(Path::new(&args[2])).unwrap(); return; }
    assert_eq!(args.len(), 2, "list_loading_bench [--create] <new-fixture-root>");
    let root = Path::new(&args[1]);
    assert_eq!(std::fs::read(root.join("fixture-version")).unwrap(), b"fldr-list-loading-v1");
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    println!("workload,mode,run,wall_us,cpu_us,allocations,peak_extra_heap_bytes,preview_us,entries,checksum");
    for (workload, files) in [("short", 20_000), ("long_unicode", 12_000)] {
        let path = root.join(workload);
        for native_only in [true, false] {
            for run in 0..8 {
                let preview_at = std::sync::Arc::new(AtomicU64::new(0));
                let preview = preview_at.clone();
                let allocations = ALLOCS.load(Relaxed);
                let live = LIVE.load(Relaxed);
                PEAK.store(live, Relaxed);
                let cpu = cpu_us();
                let started = Instant::now();
                let (count, entries) = if native_only {
                    #[cfg(windows)] {
                        let mut count = 0;
                        atlas_win_space::directory::enumerate(&path, |_| count += 1).unwrap();
                        (count, Vec::new())
                    }
                    #[cfg(not(windows))] { (std::fs::read_dir(&path).unwrap().count(), Vec::new()) }
                } else {
                    let entries = runtime.block_on(atlas_core::fs::list::list_dir_with_preview(&path,
                        false, SortColumn::Name, SortDir::Asc, FolderOrder::FoldersFirst,
                        move |entries| { assert_eq!(entries.len(), 256); preview.store(started.elapsed().as_micros() as u64, Relaxed); })).unwrap();
                    (entries.len(), entries)
                };
                let wall = started.elapsed().as_micros();
                let cpu = cpu_us() - cpu;
                let allocations = ALLOCS.load(Relaxed) - allocations;
                let peak = PEAK.load(Relaxed).saturating_sub(live);
                assert_eq!(count, files + 64 + if native_only { 8 } else { 0 });
                if !native_only {
                    assert_eq!(entries.iter().filter(|entry| entry.kind() == atlas_model::file_entry::FileKind::Dir).count(), 64);
                    assert_eq!(entries.iter().filter_map(FileEntry::size).sum::<u64>(), files as u64 * 3);
                    assert!(preview_at.load(Relaxed) > 0);
                }
                let hash = checksum(&entries); // Verification excluded from timing/allocation counters.
                if run > 0 { println!("{workload},{},{run},{wall},{cpu},{allocations},{peak},{},{count},{hash:016x}",
                    if native_only { "enumeration" } else { "listing" }, preview_at.load(Relaxed)); }
            }
        }
    }
}
