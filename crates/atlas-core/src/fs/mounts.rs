#![allow(dead_code)] // reason: retained API surface not exercised by every GPUI flow yet
use std::path::PathBuf;
use std::time::{Duration, Instant};

use std::sync::{Arc, Mutex, OnceLock};

thread_local! {
    static MOUNT_CACHE: std::cell::RefCell<Option<(Instant, Vec<PathBuf>)>> = const { std::cell::RefCell::new(None) };
}

#[derive(Default)]
struct DriveCache {
    updated: Option<Instant>,
    drives: Vec<DriveInfo>,
    pending: bool,
}

static DRIVE_CACHE: OnceLock<Arc<Mutex<DriveCache>>> = OnceLock::new();

/// A mounted volume with capacity info, used to render a fullness bar in the sidebar.
#[derive(Debug, Clone)]
pub struct DriveInfo {
    /// Human label (volume name, or the mount point basename / drive root as fallback).
    pub label: String,
    pub mount_point: PathBuf,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
    pub is_system: bool,
    pub is_network: bool,
}

impl DriveInfo {
    /// Fraction of the volume in use, `0.0..=1.0`. Zero when total is unknown.
    pub fn used_fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        let used = self.total_bytes.saturating_sub(self.available_bytes);
        (used as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
    }
}

/// Return the last mounted-drive snapshot immediately and refresh it after 5s.
pub fn list_drives() -> Vec<DriveInfo> {
    drive_snapshot(false)
}

pub fn refresh_drives() {
    drive_snapshot(true);
}

fn drive_snapshot(refresh: bool) -> Vec<DriveInfo> {
    let cache_arc = DRIVE_CACHE
        .get_or_init(|| Arc::new(Mutex::new(DriveCache::default())))
        .clone();
    let mut cache = cache_arc.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

    if !cache.pending && (refresh || cache.updated.is_none_or(|last| last.elapsed() >= Duration::from_secs(5))) {
        cache.pending = true;
        let worker_cache = cache_arc.clone();
        std::thread::spawn(move || {
            let drives = list_drives_internal();
            let mut cache = worker_cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            *cache = DriveCache { updated: Some(Instant::now()), drives, pending: false };
        });
    }
    cache.drives.clone()
}

fn list_drives_internal() -> Vec<DriveInfo> {
    let started = Instant::now();
    let drives = collect_local_drives();
    #[cfg(windows)]
    let drives = drives.into_iter().chain(collect_network_drives()).collect();
    let mut drives: Vec<_> = drives;
    drives.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    drives.dedup_by(|a, b| a.mount_point == b.mount_point);
    static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    if FIRST.swap(false, std::sync::atomic::Ordering::Relaxed) {
        tracing::info!(phase_ms = started.elapsed().as_millis(), drives = drives.len(), "startup: first drive enumeration");
    }
    drives
}

fn collect_local_drives() -> Vec<DriveInfo> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|disk| {
            let mount_point = disk.mount_point().to_path_buf();
            let name = disk.name().to_string_lossy();
            let label = if name.trim().is_empty() {
                mount_point.file_name().map_or_else(
                    || mount_point.to_string_lossy().into_owned(),
                    |s| s.to_string_lossy().into_owned(),
                )
            } else {
                name.into_owned()
            };
            let is_system = is_system_mount(&mount_point);
            let is_removable = disk.is_removable();
            DriveInfo {
                label,
                mount_point,
                total_bytes: disk.total_space(),
                available_bytes: disk.available_space(),
                is_removable,
                is_system,
                is_network: false,
            }
        })
        .collect()
}

#[cfg(windows)]
#[allow(unsafe_code)] // reason: Win32 drive enumeration and capacity queries
fn collect_network_drives() -> Vec<DriveInfo> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_CONNECTION_UNAVAIL, ERROR_MORE_DATA, NO_ERROR};
    use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    let mask = unsafe { GetLogicalDrives() };
    let remembered = remembered_network_drives();
    (b'A'..=b'Z').filter_map(|letter| {
        let assigned = mask & (1 << (letter - b'A')) != 0;
        let saved_target = remembered.get(&letter);
        if !assigned && saved_target.is_none() {
            return None;
        }
        let root = [u16::from(letter), b':' as u16, b'\\' as u16, 0];
        let drive_type = if assigned { unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } } else { 0 };
        if !matches!(drive_type, 0 | 1 | 4) {
            return None;
        }
        let mut remote_name = [0u16; 1024];
        let mut remote_len = remote_name.len() as u32;
        let drive = [u16::from(letter), b':' as u16, 0];
        let result = unsafe {
            WNetGetConnectionW(PCWSTR(drive.as_ptr()), Some(PWSTR(remote_name.as_mut_ptr())), &mut remote_len)
        };
        if drive_type != 4 && saved_target.is_none()
            && !matches!(result, NO_ERROR | ERROR_CONNECTION_UNAVAIL | ERROR_MORE_DATA) {
            return None;
        }
        let mount_point = PathBuf::from(format!("{}:\\", char::from(letter)));
        let label = if result == NO_ERROR {
            let end = remote_name.iter().position(|&unit| unit == 0).unwrap_or(0);
            String::from_utf16_lossy(&remote_name[..end])
        } else {
            saved_target.cloned().unwrap_or_else(|| "Network drive".to_string())
        };
        Some(DriveInfo {
            label, mount_point, total_bytes: 0, available_bytes: 0,
            is_removable: false, is_network: true, is_system: false,
        })
    }).collect()
}

#[cfg(windows)]
#[allow(unsafe_code)] // Windows fills an aligned buffer containing its saved mapping records.
fn remembered_network_drives() -> std::collections::HashMap<u8, String> {
    use std::mem::size_of;
    use windows::Win32::Foundation::{HANDLE, ERROR_MORE_DATA, NO_ERROR};
    use windows::Win32::NetworkManagement::WNet::{
        WNetCloseEnum, WNetEnumResourceW, WNetOpenEnumW, NETRESOURCEW,
        RESOURCETYPE_DISK, RESOURCE_REMEMBERED, WNET_OPEN_ENUM_USAGE,
    };

    struct EnumHandle(HANDLE);
    impl Drop for EnumHandle {
        fn drop(&mut self) {
            unsafe { WNetCloseEnum(self.0) };
        }
    }
    let mut handle = HANDLE::default();
    if unsafe { WNetOpenEnumW(RESOURCE_REMEMBERED, RESOURCETYPE_DISK, WNET_OPEN_ENUM_USAGE(0), None, &mut handle) } != NO_ERROR {
        return std::collections::HashMap::new();
    }
    let handle = EnumHandle(handle);
    let mut targets = std::collections::HashMap::new();
    let mut buffer = vec![0usize; 16 * 1024 / size_of::<usize>()];
    loop {
        let mut bytes = std::mem::size_of_val(buffer.as_slice()) as u32;
        let mut count = u32::MAX;
        let status = unsafe { WNetEnumResourceW(handle.0, &mut count, buffer.as_mut_ptr().cast(), &mut bytes) };
        if status == ERROR_MORE_DATA && bytes <= 256 * 1024
            && bytes as usize > std::mem::size_of_val(buffer.as_slice())
        {
            buffer.resize((bytes as usize).div_ceil(size_of::<usize>()), 0);
            continue;
        }
        if status != NO_ERROR || count == 0 || count as usize > std::mem::size_of_val(buffer.as_slice()) / size_of::<NETRESOURCEW>() {
            break;
        }
        let resources = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<NETRESOURCEW>(), count as usize) };
        for resource in resources {
            if resource.lpLocalName.is_null() || resource.lpRemoteName.is_null() {
                continue;
            }
            let Ok(drive) = (unsafe { resource.lpLocalName.to_string() }) else { continue };
            let [letter, b':'] = drive.as_bytes() else { continue };
            if letter.is_ascii_alphabetic() {
                if let Ok(remote) = unsafe { resource.lpRemoteName.to_string() } {
                    targets.insert(letter.to_ascii_uppercase(), remote);
                }
            }
        }
    }
    targets
}

#[inline]
fn is_system_mount(mount_point: &std::path::Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        if let Ok(sys_drive) = std::env::var("SystemDrive") {
            let p_str = mount_point.to_string_lossy();
            if p_str.starts_with(&sys_drive) {
                return true;
            }
        }
        let s = mount_point.to_string_lossy();
        s.starts_with("C:") || s.starts_with("c:")
    }
    #[cfg(not(target_os = "windows"))]
    {
        mount_point == std::path::Path::new("/")
    }
}

pub fn read_mounts() -> Vec<PathBuf> {
    MOUNT_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((last_update, mounts)) = &*cache {
            if last_update.elapsed() < Duration::from_secs(5) {
                return mounts.clone();
            }
        }

        let new_mounts = read_mounts_internal();
        *cache = Some((Instant::now(), new_mounts.clone()));
        new_mounts
    })
}

/// Linux: parse `/proc/mounts` and keep only real on-disk filesystems (skipping the
/// many virtual/pseudo mounts the kernel exposes).
#[cfg(target_os = "linux")]
fn read_mounts_internal() -> Vec<PathBuf> {
    let content = match std::fs::read_to_string("/proc/mounts") {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let real_fstypes = [
        "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "jfs", "reiserfs", "ntfs", "ntfs-3g",
        "vfat", "exfat", "fuseblk", "zfs",
    ];
    content
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _dev = parts.next()?;
            let mountpoint = parts.next()?;
            let fstype = parts.next()?;
            real_fstypes
                .contains(&fstype)
                .then(|| PathBuf::from(mountpoint))
        })
        .collect()
}

/// Windows / macOS: enumerate mounted volumes via `sysinfo` (already a dependency and
/// cross-platform). On Windows these are drive roots like `C:\`; on macOS they are the
/// root `/` plus `/Volumes/*` for external disks.
#[cfg(not(target_os = "linux"))]
fn read_mounts_internal() -> Vec<PathBuf> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut mounts: Vec<PathBuf> = disks
        .list()
        .iter()
        .map(|disk| disk.mount_point().to_path_buf())
        .collect();
    mounts.sort();
    mounts.dedup();
    mounts
}
