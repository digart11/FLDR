//! Filesystem navigation only: no enumeration, synthetic FileEntry or statistics work.
use atlas_model::{location::Location, settings::ViewMode};
use std::path::{Component, Path, PathBuf};

pub(crate) fn parent_path(location: &Location, mode: &ViewMode) -> Option<PathBuf> {
    if !matches!(mode, ViewMode::Details | ViewMode::List | ViewMode::Grid) {
        return None;
    }
    let path = location.fs_dir()?;
    let parent = location.ascend()?;
    let parent_path = parent.display_path();
    (!parent_path.as_os_str().is_empty() && parent_path.as_path() != path).then_some(parent_path)
}

pub(crate) fn is_address_shortcut(
    key: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    platform: bool,
) -> bool {
    !shift && !platform && ((key == "l" && ctrl && !alt) || (key == "d" && alt && !ctrl))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ParentKey {
    Open,
    Leave,
    LeaveAndForward,
    Other,
}

pub(crate) fn parent_key(key: &str, ctrl: bool, alt: bool, platform: bool) -> ParentKey {
    match key {
        "enter" if !ctrl && !alt && !platform => ParentKey::Open,
        "escape" => ParentKey::Leave,
        "up" | "down" | "left" | "right" | "home" | "end" | "pageup" | "pagedown" => {
            ParentKey::LeaveAndForward
        }
        _ => ParentKey::Other,
    }
}

/// Resolve text lexically. The existing asynchronous navigation pipeline handles
/// existence and access errors, including disconnected UNC shares.
pub(crate) fn edited_path(text: &str, base: &Path) -> Result<PathBuf, &'static str> {
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(text);
    if text.is_empty() || text.chars().any(|c| matches!(c, '\0' | '\r' | '\n' | '"')) {
        return Err("Enter a directory path");
    }
    if text.contains("://") {
        return Err("Enter a filesystem path");
    }
    let drive = text.as_bytes().get(1) == Some(&b':') && text.as_bytes()[0].is_ascii_alphabetic();
    let drive_root;
    let text = if drive && text.len() == 2 {
        drive_root = format!("{text}\\");
        &drive_root
    } else {
        if drive && !matches!(text.as_bytes().get(2), Some(b'\\' | b'/')) {
            return Err("Use an absolute drive path, such as C:\\");
        }
        text
    };
    let path = Path::new(text);
    #[cfg(windows)]
    if let Some(server) = atlas_model::location::network_server_root(path) {
        return Ok(PathBuf::from(format!(r"\\{server}")));
    }
    #[cfg(windows)]
    if let [first, second, ..] = text.as_bytes() {
        if [first, second].into_iter().all(|byte| matches!(byte, b'\\' | b'/')) {
            use std::path::Prefix;
            match path.components().next() {
                Some(Component::Prefix(prefix)) if path.is_absolute() => match prefix.kind() {
                    Prefix::UNC(..) => {}
                    Prefix::VerbatimUNC(..) | Prefix::VerbatimDisk(_) => return Ok(path.to_path_buf()),
                    _ => return Err("Enter a filesystem path"),
                },
                _ => return Err("Enter a network path with a share name"),
            }
        }
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}

#[cfg(all(test, windows))]
#[test]
fn address_keeps_unc_roots_and_verbatim_paths() {
    let base = Path::new(r"C:\Local");
    for path in [r"\\server\share", r"\\server\share\folder", r"\\?\UNC\server\share\folder"] {
        assert_eq!(edited_path(path, base).unwrap(), PathBuf::from(path));
    }
    assert_eq!(edited_path(r"\\server\share\..", base).unwrap(), PathBuf::from(r"\\server\share"));
    assert_eq!(edited_path(r"\\server", base).unwrap(), PathBuf::from(r"\\server"));
    assert!(edited_path(r"\\.\PhysicalDrive0", base).is_err());
}
