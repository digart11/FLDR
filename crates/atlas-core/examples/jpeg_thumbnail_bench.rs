//! Disposable photo benchmark. Run --create <new-root> <source-photo>, or <root> <file> <edge> <orient>.
#![allow(unsafe_code)]
use std::{alloc::{GlobalAlloc, Layout, System}, sync::atomic::{AtomicU64, Ordering::Relaxed}, path::Path, time::Instant};
struct Allocator;
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() { let n = LIVE.fetch_add(l.size() as u64, Relaxed) + l.size() as u64; PEAK.fetch_max(n, Relaxed); }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { LIVE.fetch_sub(l.size() as u64, Relaxed); unsafe { System.dealloc(p, l); } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() { LIVE.fetch_sub(l.size() as u64, Relaxed); let live = LIVE.fetch_add(n as u64, Relaxed) + n as u64; PEAK.fetch_max(live, Relaxed); }
        q
    }
}
#[global_allocator] static ALLOC: Allocator = Allocator;
#[cfg(windows)]
fn counters() -> (u64, usize) {
    #[repr(C)] struct Memory { cb: u32, faults: u32, peak_ws: usize, ws: usize, rest: [usize; 6] }
    #[link(name="kernel32")] extern "system" {
        fn GetProcessTimes(p: isize, c: *mut u64, e: *mut u64, k: *mut u64, u: *mut u64) -> i32;
        fn K32GetProcessMemoryInfo(p: isize, memory: *mut Memory, cb: u32) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (0,0,0,0);
    let mut memory = Memory { cb: std::mem::size_of::<Memory>() as u32, faults: 0, peak_ws: 0, ws: 0, rest: [0;6] };
    assert_ne!(unsafe { GetProcessTimes(-1, &mut c, &mut e, &mut k, &mut u) }, 0);
    assert_ne!(unsafe { K32GetProcessMemoryInfo(-1, &mut memory, memory.cb) }, 0);
    ((k + u) / 10, memory.peak_ws)
}
#[cfg(not(windows))] fn counters() -> (u64, usize) { (0,0) }

fn create(root: &Path, source: &Path) {
    std::fs::create_dir(root).unwrap();
    let image = image::open(source).unwrap();
    let photo = image.resize_to_fill(6000, 4000, image::imageops::FilterType::Lanczos3);
    for (name, pixels) in [("landscape.jpg", photo.clone()), ("portrait.jpg", photo.rotate90()), ("gray.jpg", image::DynamicImage::ImageLuma8(photo.to_luma8()))] {
        let mut file = std::fs::File::create(root.join(name)).unwrap();
        if name == "gray.jpg" {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file,90).encode(pixels.to_luma8().as_raw(),pixels.width(),pixels.height(),image::ExtendedColorType::L8).unwrap();
        } else { image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, 90).encode_image(&pixels).unwrap(); }
    }
    std::fs::copy(source, root.join("profiled.jpg")).unwrap();
    // Independent progressive fallback control: retain the original compressed
    // scan while removing only its ICC APP2 metadata from this disposable copy.
    let source_bytes = std::fs::read(source).unwrap();
    let mut unprofiled = source_bytes[..2].to_vec();
    let mut at = 2;
    while at + 4 < source_bytes.len() {
        assert_eq!(source_bytes[at], 0xff);
        let marker = source_bytes[at+1];
        if marker == 0xda { unprofiled.extend_from_slice(&source_bytes[at..]); break; }
        let len = u16::from_be_bytes([source_bytes[at+2],source_bytes[at+3]]) as usize;
        let end = at + 2 + len;
        assert!(len >= 2 && end <= source_bytes.len());
        if marker != 0xe2 || !source_bytes[at+4..end].starts_with(b"ICC_PROFILE\0") { unprofiled.extend_from_slice(&source_bytes[at..end]); }
        at = end;
    }
    std::fs::write(root.join("progressive.jpg"),unprofiled).unwrap();
    let bytes = std::fs::read(root.join("landscape.jpg")).unwrap();
    let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend_from_slice(&[6,0,0,0,0,0,0,0]);
    let mut portrait = vec![0xff,0xd8,0xff,0xe1];
    portrait.extend_from_slice(&((exif.len()+2) as u16).to_be_bytes());
    portrait.extend(exif);portrait.extend_from_slice(&bytes[2..]);
    std::fs::write(root.join("exif6.jpg"),portrait).unwrap();
    std::fs::write(root.join("fixture-version"), b"fldr-jpeg-v1").unwrap();
}
fn main() {
    let a: Vec<_> = std::env::args().collect();
    if a.len()==4 && a[1]=="--create" { create(Path::new(&a[2]),Path::new(&a[3])); return; }
    assert_eq!(a.len(),5);
    let root = Path::new(&a[1]);
    assert_eq!(std::fs::read(root.join("fixture-version")).unwrap(), b"fldr-jpeg-v1");
    let edge: u32 = a[3].parse().unwrap();
    let orient: bool = a[4].parse().unwrap();
    let live = LIVE.load(Relaxed); PEAK.store(live,Relaxed);
    println!("file,edge,run,wall_us,cpu_us,peak_extra_rust_heap,peak_working_set,width,height");
    for run in 0..6 {
        let cpu = counters().0; let start = Instant::now();
        let (rgba,w,h) = atlas_core::fs::preview::raster_thumbnail(&root.join(&a[2]),edge,orient).unwrap();
        let wall = start.elapsed().as_micros(); let (after,ws) = counters();
        assert_eq!(rgba.len(),w*h*4);
        if run==0 { image::save_buffer(root.join(format!("{}-{}-{}.png",a[2],edge,std::env::var("FLDR_BENCH_LABEL").unwrap_or("output".into()))),&rgba,w as u32,h as u32,image::ColorType::Rgba8).unwrap(); }
        else { println!("{},{edge},{run},{wall},{},{},{ws},{w},{h}",a[2],after-cpu,PEAK.load(Relaxed).saturating_sub(live)); }
    }
}
