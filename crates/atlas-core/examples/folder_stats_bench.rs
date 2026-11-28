//! Disposable, deterministic scanner benchmark. No release build is required.
//! --create <new-root> creates fixtures; <root> scans the same fixtures seven times.
#![allow(unsafe_code)]
use std::{alloc::{GlobalAlloc, Layout, System}, path::Path, sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed}, time::Instant};

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
fn cpu_ns() -> u64 {
    #[link(name = "kernel32")]
    extern "system" { fn GetProcessTimes(process: isize, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> i32; }
    let (mut created, mut exited, mut kernel, mut user) = (0, 0, 0, 0);
    assert_ne!(unsafe { GetProcessTimes(-1, &mut created, &mut exited, &mut kernel, &mut user) }, 0);
    (kernel + user) * 100
}
#[cfg(not(windows))]
fn cpu_ns() -> u64 { 0 }

fn create(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir(root)?; // Never populate an existing directory.
    std::fs::write(root.join("fixture-version"), b"fileop-folder-stats-v1")?;
    let flat = root.join("small");
    std::fs::create_dir(&flat)?;
    for index in 0..12_000 { std::fs::write(flat.join(format!("file-{index:05}")), [7; 32])?; }
    let nested = root.join("nested");
    for index in 0..128 {
        let dir = nested.join(format!("branch-{}", index / 16)).join(format!("leaf-{index}"));
        std::fs::create_dir_all(&dir)?;
        for file in 0..32 { std::fs::write(dir.join(format!("file-{file}")), vec![3; file + 1])?; }
        std::fs::create_dir(dir.join("empty"))?;
    }
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3 && args[1] == "--create" { create(Path::new(&args[2])).unwrap(); return; }
    assert_eq!(args.len(), 2, "usage: folder_stats_bench [--create] <fixture-root>");
    let root = Path::new(&args[1]);
    assert_eq!(std::fs::read(root.join("fixture-version")).unwrap(), b"fileop-folder-stats-v1");
    println!("workload,mode,run,wall_us,cpu_us,allocations,peak_extra_heap_bytes,files,folders,bytes,descendants");
    for workload in ["small", "nested"] {
        for automatic in [false, true] {
            for run in 0..8 { // Run zero warms caches; omit it from the output.
                let path = root.join(workload);
                let cancel = AtomicBool::new(false);
                let descendants = AtomicU64::new(0);
                let allocs = ALLOCS.load(Relaxed);
                let live = LIVE.load(Relaxed);
                PEAK.store(live, Relaxed);
                let cpu = cpu_ns();
                let wall = Instant::now();
                let report = if automatic {
                    atlas_core::fs::folder_size::scan_automatic(vec![path], &cancel, 1, |_| {}, |_| { descendants.fetch_add(1, Relaxed); })
                } else {
                    atlas_core::fs::folder_size::scan_with_stats(vec![path], &cancel, |_| false, |_| {}, |_| { descendants.fetch_add(1, Relaxed); })
                };
                let elapsed = wall.elapsed().as_micros();
                let cpu = (cpu_ns() - cpu) / 1000;
                let allocs = ALLOCS.load(Relaxed) - allocs;
                let peak = PEAK.load(Relaxed).saturating_sub(live);
                assert!(report.done && !report.cancelled && report.errors == 0 && report.skipped == 0, "{report:?}");
                let expected = if workload == "small" { (12_000, 0, 384_000, 1) } else { (4096, 264, 67_584, 265) };
                assert_eq!((report.files, report.folders, report.bytes, descendants.load(Relaxed)), expected);
                if run > 0 { println!("{workload},{},{run},{elapsed},{cpu},{allocs},{peak},{},{},{},{}", if automatic { "automatic" } else { "raw" }, report.files, report.folders, report.bytes, descendants.load(Relaxed)); }
            }
        }
    }
}
