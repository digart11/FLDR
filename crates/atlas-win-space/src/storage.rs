//! Metadata-only volume identity and media capabilities.
use std::{
    ffi::OsStr,
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{
        CreateFileW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, GetVolumePathNameW,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    },
    System::{
        Ioctl::{
            PropertyStandardQuery, StorageDeviceSeekPenaltyProperty,
            DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY,
        },
        IO::DeviceIoControl,
    },
};

pub fn volume_serial(root: &Path) -> Option<u32> {
    let root = super::wide(root.as_os_str());
    let mut serial = 0;
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            null_mut(),
            0,
            &mut serial,
            null_mut(),
            null_mut(),
            null_mut(),
            0,
        )
    };
    (ok != 0).then_some(serial)
}

pub fn mounted_volume_serials() -> Vec<(PathBuf, u32)> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|bit| mask & (1 << bit) != 0)
        .filter_map(|bit| {
            let letter = char::from(b'A' + bit as u8);
            let wide_root = [letter as u16, b':' as u16, b'\\' as u16, 0];
            if unsafe { GetDriveTypeW(wide_root.as_ptr()) } == 4 {
                return None;
            }
            let root = PathBuf::from(format!("{letter}:\\"));
            Some((root.clone(), volume_serial(&root)?))
        })
        .collect()
}

/// `None` means an unsupported/remote device: callers should use conservative I/O.
pub fn incurs_seek_penalty(path: &Path) -> Option<bool> {
    let path = super::wide(path.as_os_str());
    let mut root = [0u16; 32768];
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return None;
    }
    let root_len = root.iter().position(|unit| *unit == 0)?;
    let root = String::from_utf16_lossy(&root[..root_len]);
    // Drive-letter volumes support the zero-access storage property query.
    let bytes = root.as_bytes();
    if bytes.len() != 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    let device = super::wide(OsStr::new(&format!(r"\\.\{}:", char::from(bytes[0]))));
    let handle = unsafe {
        CreateFileW(
            device.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let handle = super::Handle(handle);
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceSeekPenaltyProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut descriptor: DEVICE_SEEK_PENALTY_DESCRIPTOR = unsafe { zeroed() };
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&raw const query).cast(),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            (&raw mut descriptor).cast(),
            size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
            &mut returned,
            null_mut(),
        )
    };
    (ok != 0 && returned as usize >= size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()
        && descriptor.Size as usize >= size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>())
        .then_some(descriptor.IncursSeekPenalty != 0)
}

/// Worker-only classification, including mapped network drives and mount points.
pub fn statistics_storage(path: &Path) -> atlas_model::automatic_stats::StorageKind {
    use atlas_model::automatic_stats::StorageKind;
    // Recognize shares and mapped drives before asking Windows to resolve a
    // volume path. Reading a retained share snapshot must not trigger a server
    // lookup merely to discover that automatic network scans are disabled.
    if let Some(std::path::Component::Prefix(prefix)) = path.components().next() {
        if matches!(prefix.kind(), std::path::Prefix::UNC(..) | std::path::Prefix::VerbatimUNC(..)) {
            return if path.components().count() <= 2 { StorageKind::Unknown } else { StorageKind::Network };
        }
        if let std::path::Prefix::Disk(drive) | std::path::Prefix::VerbatimDisk(drive) = prefix.kind() {
            let root = [u16::from(drive), 58, 92, 0];
            if unsafe { GetDriveTypeW(root.as_ptr()) } == 4 {
                return if path.components().count() <= 2 { StorageKind::Unknown } else { StorageKind::Network };
            }
        }
    }
    let wide = super::wide(path.as_os_str());
    let mut root = [0u16; 32768];
    if unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return StorageKind::Unknown;
    }
    let len = root.iter().position(|unit| *unit == 0).unwrap_or(0);
    use std::os::windows::ffi::OsStringExt;
    let volume_root = PathBuf::from(std::ffi::OsString::from_wide(&root[..len]));
    if atlas_model::folder_stats::key(path) == atlas_model::folder_stats::key(&volume_root) {
        return StorageKind::Unknown; // Never scan a whole mounted volume automatically.
    }
    match unsafe { GetDriveTypeW(root.as_ptr()) } {
        4 => StorageKind::Network,
        3 => atlas_model::automatic_stats::fixed_storage(incurs_seek_penalty(path)),
        _ => StorageKind::Unknown,
    }
}

/// Restored on drop: Tokio reuses blocking threads for unrelated work.
pub struct BackgroundStatisticsPriority(bool);
impl BackgroundStatisticsPriority {
    pub fn enter() -> Self {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority};
        let entered = unsafe { SetThreadPriority(GetCurrentThread(), 0x00010000) } != 0;
        Self(entered)
    }
}
impl Drop for BackgroundStatisticsPriority {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority};
        if self.0 { unsafe { SetThreadPriority(GetCurrentThread(), 0x00020000); } }
    }
}
