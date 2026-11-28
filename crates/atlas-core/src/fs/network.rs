//! Enumerate an explicitly requested SMB server, without opening any shares.
use atlas_model::file_entry::FileEntry;
use std::io;

#[cfg(not(windows))]
pub fn shares(_server: &str) -> io::Result<Vec<FileEntry>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "SMB server browsing requires Windows",
    ))
}

#[cfg(windows)]
#[allow(unsafe_code)] // Win32 owns and sizes the paged share information buffer.
pub fn shares(server: &str) -> io::Result<Vec<FileEntry>> {
    use atlas_model::file_entry::{FileEntryData, FileKind};
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::ptr::null_mut;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{ERROR_MORE_DATA, NO_ERROR};
    use windows::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
    use windows::Win32::Storage::FileSystem::{NetShareEnum, SHARE_INFO_1};

    struct Buffer(*mut u8);
    impl Drop for Buffer {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { NetApiBufferFree(Some(self.0.cast::<c_void>())) };
            }
        }
    }
    let root = format!(r"\\{server}");
    if atlas_model::location::network_server_root(Path::new(&root)).as_deref() != Some(server) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid SMB server name",
        ));
    }
    let server = HSTRING::from(&root);
    let mut resume = 0;
    let mut entries = Vec::new();
    loop {
        let mut data = Buffer(null_mut());
        let (mut count, mut total) = (0, 0);
        let previous_resume = resume;
        let status = unsafe {
            NetShareEnum(
                &server,
                1,
                &mut data.0,
                64 * 1024,
                &mut count,
                &mut total,
                Some(&mut resume),
            )
        };
        if status != NO_ERROR.0 && status != ERROR_MORE_DATA.0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        if count > 0 {
            if data.0.is_null() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Missing share list buffer",
                ));
            }
            let shares = unsafe {
                std::slice::from_raw_parts(data.0.cast::<SHARE_INFO_1>(), count as usize)
            };
            for share in shares {
                // Omit non-disk, administrative and explicitly hidden shares.
                if (share.shi1_type.0 as u32) & 0x8000_00ff != 0 || share.shi1_netname.is_null() {
                    continue;
                }
                let Ok(name) = (unsafe { share.shi1_netname.to_string() }) else {
                    continue;
                };
                if name.is_empty()
                    || matches!(name.as_str(), "." | "..")
                    || name.ends_with('$')
                    || name.contains(['\\', '/', ':'])
                    || name.chars().any(char::is_control)
                {
                    continue;
                }
                entries.push(
                    FileEntryData {
                        path: PathBuf::from(format!(r"{root}\{name}\")),
                        name: name.into(),
                        kind: FileKind::Dir,
                        size: None,
                        modified: None,
                        _is_hidden: false,
                        _is_symlink: false,
                        archive_root: None,
                        recycle_info: None,
                    }
                    .into(),
                );
            }
        }
        if status == NO_ERROR.0 {
            break;
        }
        if resume == previous_resume {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Share enumeration did not advance",
            ));
        }
    }
    entries.sort_unstable_by(|a: &FileEntry, b| a.name.cmp(&b.name));
    entries.dedup_by(|a, b| a.name == b.name);
    Ok(entries)
}
