use crate::tag_cache::TagId;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Location {
    Directory(PathBuf),
    Archive {
        archive: PathBuf,
        inner: PathBuf,
    },
    Tag(TagId),
    RecycleBin,
    /// A Windows SMB server's share list, which is not a filesystem directory.
    NetworkServer(String),
    #[cfg(feature = "provider-host")]
    Remote {
        id: String,
        path: String,
    },
}

impl Location {
    pub fn from_path(path: PathBuf) -> Self {
        #[cfg(windows)]
        if let Some(server) = network_server_root(&path) {
            return Self::NetworkServer(server);
        }
        Self::Directory(path)
    }

    /// Real filesystem directory for this location; `None` for archives, tags, remotes.
    /// Use for: filesystem watchers, DnD drops, paste destinations, goto-root.
    pub fn fs_dir(&self) -> Option<&Path> {
        match self {
            Self::Directory(p) => Some(p),
            _ => None,
        }
    }

    /// Path suitable for breadcrumbs and title bars.
    pub fn display_path(&self) -> PathBuf {
        match self {
            Self::Directory(p) => p.clone(),
            Self::Archive { archive, inner } => {
                if inner == Path::new("") {
                    archive.clone()
                } else {
                    archive.join(inner)
                }
            }
            Self::Tag(id) => PathBuf::from(format!("tag:{}", id.0)),
            Self::RecycleBin => PathBuf::from("Recycle Bin"),
            Self::NetworkServer(server) => PathBuf::from(format!(r"\\{server}")),
            #[cfg(feature = "provider-host")]
            Self::Remote { id, path } => PathBuf::from(format!("{id}:{}", crate::remote_path::display(path))),
        }
    }

    /// `(archive_file, inner_path)` when browsing inside an archive, else `None`.
    pub fn as_archive(&self) -> Option<(&Path, &Path)> {
        match self {
            Self::Archive { archive, inner } => Some((archive, inner)),
            _ => None,
        }
    }

    pub fn is_directory(&self) -> bool {
        matches!(self, Self::Directory(_))
    }

    pub fn is_archive(&self) -> bool {
        matches!(self, Self::Archive { .. })
    }

    /// Navigate one level up, returning the parent `Location`.
    /// Returns `None` at filesystem roots and for tags/remotes.
    pub fn ascend(&self) -> Option<Location> {
        match self {
            Self::Directory(p) => {
                #[cfg(windows)]
                if let Some(server) = network_server_for_share_root(p) {
                    return Some(Self::NetworkServer(server));
                }
                let parent = p.parent()?;
                if parent == p.as_path() {
                    return None;
                }
                Some(Self::Directory(parent.to_path_buf()))
            }
            Self::Archive { archive, inner } => {
                if inner == Path::new("") {
                    Some(Self::Directory(archive.parent()?.to_path_buf()))
                } else {
                    let parent_inner = inner.parent().unwrap_or(Path::new("")).to_path_buf();
                    Some(Self::Archive {
                        archive: archive.clone(),
                        inner: parent_inner,
                    })
                }
            }
            Self::Tag(_) | Self::RecycleBin | Self::NetworkServer(_) => None,
            #[cfg(feature = "provider-host")]
            Self::Remote { id, path } => {
                let trimmed = path.trim_matches('/');
                if trimmed.is_empty() {
                    None
                } else {
                    let parent = match trimmed.rsplit_once('/') {
                        Some((p, _)) => p.to_string(),
                        None => String::new(),
                    };
                    Some(Self::Remote {
                        id: id.clone(),
                        path: parent,
                    })
                }
            }
        }
    }
}

/// Parse a bare Windows UNC authority. A share or device path is a different target.
pub fn network_server_root(path: &Path) -> Option<String> {
    let path = path.to_str()?;
    if !path.starts_with(['\\', '/']) { return None; }
    let text = path.replace('/', "\\");
    let mut body = text.strip_prefix(r"\\")?;
    if let Some(verbatim) = body.strip_prefix(r"?\") {
        let (namespace, authority) = verbatim.split_once('\\')?;
        if !namespace.eq_ignore_ascii_case("UNC") { return None; }
        body = authority;
    }
    let body = body.trim_end_matches('\\');
    valid_server(body).then(|| body.to_string())
}

fn valid_server(server: &str) -> bool {
    !server.is_empty() && server.len() <= 255
        && server.bytes().any(|byte| byte.is_ascii_alphanumeric())
        && server.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        && !server.eq_ignore_ascii_case("wsl.localhost")
}

/// An SMB server or share root is a navigation target, never a file-operation source.
pub fn is_network_root(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let path_bytes = path.as_os_str().as_encoded_bytes();
        if !matches!(path_bytes, [b'\\' | b'/', b'\\' | b'/', ..]) {
            return false;
        }
        network_server_root(path).is_some() || network_server_for_share_root(path).is_some()
    }
    #[cfg(not(windows))]
    { let _ = path; false }
}

#[cfg(windows)]
fn network_server_for_share_root(path: &Path) -> Option<String> {
    use std::path::{Component, Prefix};
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else { return None };
    let (Prefix::UNC(server, _) | Prefix::VerbatimUNC(server, _)) = prefix.kind() else { return None };
    let server = server.to_str()?;
    (valid_server(server) && components.all(|part| part == Component::RootDir))
        .then(|| server.to_string())
}

impl Default for Location {
    fn default() -> Self {
        Self::Directory(PathBuf::new())
    }
}

#[cfg(all(test, feature = "provider-host"))]
mod tests {
    use super::*;

    #[test]
    fn test_remote_ascend() {
        let nested = Location::Remote {
            id: "drive".into(),
            path: "folder/subfolder".into(),
        };
        let parent = nested.ascend().unwrap();
        assert_eq!(
            parent,
            Location::Remote {
                id: "drive".into(),
                path: "folder".into(),
            }
        );

        let root_parent = parent.ascend().unwrap();
        assert_eq!(
            root_parent,
            Location::Remote {
                id: "drive".into(),
                path: String::new(),
            }
        );

        assert_eq!(root_parent.ascend(), None);
    }
}
