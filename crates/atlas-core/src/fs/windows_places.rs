//! Worker-only discovery using local Windows and desktop-client metadata.
#![allow(unsafe_code)] // reason: this module is the Windows discovery adapter
use atlas_model::places::{Place, PlaceTarget};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDriveTypeW;

pub fn discover() -> Vec<Place> {
    let mut places = Vec::new();
    for app_data in [dirs::config_dir(), dirs::data_local_dir()]
        .into_iter()
        .flatten()
    {
        let Some(data) = read_small(&app_data.join("Dropbox").join("info.json")) else {
            continue;
        };
        places.extend(dropbox_places(&data));
    }

    // The Sync Client also supports a custom ccnet directory and data directory.
    let data_dir = std::env::var_os("SEAFILE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let config = std::env::var_os("CCNET_CONF_DIR")
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|home| home.join("ccnet")))?;
            let data = read_small(&config.join("seafile.ini"))?;
            Some(PathBuf::from(
                std::str::from_utf8(&data)
                    .ok()?
                    .trim_start_matches('\u{feff}')
                    .lines()
                    .next()?,
            ))
        });
    if let Some(dir) = data_dir.filter(|path| local_drive(path)) {
        let db = dir.join("repo.db");
        places.extend(seafile_worktrees(&db).into_iter().map(|path| {
            let label = format!("Seafile — {}", crate::util::display::display_name(&path));
            place(label, path, "seafile")
        }));
    }
    places.extend(registered_roots());
    places.retain(|entry| match &entry.target {
        PlaceTarget::Folder(path) => local_drive(path) && path.is_dir(),
        PlaceTarget::RecycleBin => false,
    });
    places.extend(network_shortcuts());
    let mut seen = HashSet::new();
    places.retain(|entry| entry.key().is_some_and(|key| seen.insert(key)));
    places
}

fn place(label: impl Into<String>, path: PathBuf, icon: &str) -> Place {
    Place {
        label: label.into(),
        target: PlaceTarget::Folder(path),
        icon: icon.into(),
        hidden: false,
    }
}

fn read_small(path: &Path) -> Option<Vec<u8>> {
    if !local_drive(path) {
        return None;
    }
    let mut data = Vec::new();
    File::open(path)
        .ok()?
        .take(64 * 1024 + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() <= 64 * 1024).then_some(data)
}

fn local_drive(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    if !path.is_absolute() {
        return false;
    }
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let (Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)) = prefix.kind() else {
        return false;
    };
    let root = [u16::from(letter), b':' as u16, b'\\' as u16, 0];
    matches!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, 2 | 3)
}

fn dropbox_places(data: &[u8]) -> Vec<Place> {
    let Ok(info) = serde_json::from_slice::<serde_json::Value>(data) else {
        return Vec::new();
    };
    [("personal", "Dropbox"), ("business", "Dropbox (Business)")]
        .into_iter()
        .filter_map(|(account, label)| {
            let path = PathBuf::from(info.get(account)?.get("path")?.as_str()?);
            path.is_absolute().then(|| place(label, path, "dropbox"))
        })
        .collect()
}

fn seafile_worktrees(db: &Path) -> Vec<PathBuf> {
    // Open only the local repository database and select the worktree property.
    let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Vec::new();
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(100));
    let Ok(mut query) = conn.prepare(
        "SELECT DISTINCT p.value FROM RepoProperty p
         JOIN Repo r ON r.repo_id = p.repo_id
         WHERE p.key = 'worktree'
           AND NOT EXISTS (SELECT 1 FROM DeletedRepo d WHERE d.repo_id = p.repo_id)
         ORDER BY p.value",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = query.query_map([], |row| row.get::<_, String>(0).map(PathBuf::from)) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok)
        .filter(|p| p.is_absolute())
        .collect()
}

fn registered_roots() -> Vec<Place> {
    use windows::Storage::Provider::StorageProviderSyncRootManager;
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }
    if unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_err() {
        return Vec::new();
    }
    let _apartment = Apartment;
    let Ok(roots) = StorageProviderSyncRootManager::GetCurrentSyncRoots() else {
        return Vec::new();
    };
    roots
        .into_iter()
        .filter_map(|root| {
            let id = root.Id().ok()?.to_string();
            let provider = id.split('!').next()?;
            let (label, icon) = match provider.to_ascii_lowercase().as_str() {
                "dropbox" => ("Dropbox", "dropbox"),
                "seafile" => ("Seafile", "seafile"),
                _ => return None,
            };
            let path = PathBuf::from(root.Path().ok()?.Path().ok()?.to_os_string());
            Some(place(label, path, icon))
        })
        .collect()
}

fn network_shortcuts() -> Vec<Place> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_NetHood, SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY};
    let Ok(raw) = (unsafe { SHGetKnownFolderPath(&FOLDERID_NetHood, KF_FLAG_DONT_VERIFY, None) })
    else {
        return Vec::new();
    };
    let dir = PathBuf::from(unsafe { std::ffi::OsString::from_wide(raw.as_wide()) });
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    if !local_drive(&dir) {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut locations: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let ty = entry.file_type().ok()?;
            let mut link = entry.path();
            let label = if ty.is_dir() {
                link.push("target.lnk");
                entry.file_name().to_string_lossy().into_owned()
            } else if ty.is_file()
                && link
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
            {
                link.file_stem()?.to_string_lossy().into_owned()
            } else {
                return None;
            };
            let path = PathBuf::from(atlas_win_space::metadata::shortcut_target(&link)?);
            // Filesystem shares only. Reading a shortcut never probes the target.
            if !matches!(path.components().next(), Some(std::path::Component::Prefix(p))
            if matches!(p.kind(), std::path::Prefix::UNC(..) | std::path::Prefix::VerbatimUNC(..)))
            {
                return None;
            }
            Some(place(label, path, "globe-simple"))
        })
        .collect();
    locations.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.key().cmp(&b.key())));
    locations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropbox_reads_only_documented_account_paths() {
        let places = dropbox_places(
            br#"{
            "personal":{"path":"D:\\Dropbox Person","host":123},
            "business":{"path":"E:\\Team"},
            "unknown":{"path":"C:\\ignored"}
        }"#,
        );
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].key().as_deref(), Some(r"d:\dropbox person"));
        assert_eq!(places[1].label, "Dropbox (Business)");
        assert!(dropbox_places(br#"{"personal":{"path":"relative"}}"#).is_empty());
        assert!(dropbox_places(b"{garbage").is_empty());
    }

    #[test]
    fn seafile_reads_active_worktrees_without_creating_missing_databases() {
        let fixture = tempfile::tempdir().unwrap();
        let db = fixture.path().join("repo.db");
        assert!(seafile_worktrees(&db).is_empty());
        assert!(!db.exists());
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE Repo (repo_id TEXT PRIMARY KEY);
            CREATE TABLE RepoProperty (repo_id TEXT, key TEXT, value TEXT);
            CREATE TABLE DeletedRepo (repo_id TEXT PRIMARY KEY);
            INSERT INTO Repo VALUES ('active'), ('deleted'), ('relative');
            INSERT INTO DeletedRepo VALUES ('deleted');
            INSERT INTO RepoProperty VALUES
                ('active', 'worktree', 'D:\Existing Library'),
                ('active', 'other', 'D:\Ignored'),
                ('deleted', 'worktree', 'D:\Deleted'),
                ('relative', 'worktree', 'relative'),
                ('orphan', 'worktree', 'D:\Orphan');
        "#,
        )
        .unwrap();
        drop(conn);
        let before = fs::read(&db).unwrap();
        assert_eq!(
            seafile_worktrees(&db),
            [PathBuf::from(r"D:\Existing Library")]
        );
        assert_eq!(fs::read(&db).unwrap(), before);
    }
}
