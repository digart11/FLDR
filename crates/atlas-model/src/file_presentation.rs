//! Pure filename and clipboard presentation; never canonicalize or probe paths.
use std::path::{Path, PathBuf};

/// Leading-dot names alone are extensionless; retain the actual suffix's case.
pub fn extension(name: &str) -> Option<&str> {
    let (stem, suffix) = name.rsplit_once('.')?;
    (!stem.is_empty() && !suffix.is_empty() && !stem.chars().all(|c| c == '.')).then_some(suffix)
}

/// Only actual filesystem files have an extension pill.
pub fn selected_extension(
    selected: &[crate::file_entry::FileEntry],
    filesystem: bool,
) -> Option<&str> {
    let [entry] = selected else {
        return None;
    };
    (filesystem && entry.kind() == crate::file_entry::FileKind::File)
        .then(|| extension(&entry.name))
        .flatten()
}

/// Exactly one absolute filesystem path. Unicode text, without shell quoting.
pub fn copy_path(paths: &[PathBuf]) -> Option<String> {
    let [path] = paths else {
        return None;
    };
    path_text(path)
}

pub fn path_text(path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let text = path.to_str()?;
    // Ancestors identify valid roots without stripping the UNC prefix or root slash.
    let root = path.parent().is_none();
    Some(if root {
        text.to_owned()
    } else {
        text.trim_end_matches(['\\', '/']).to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_extensions() {
        for (name, expected) in [
            ("settings.json", Some("json")),
            ("notes.txt", Some("txt")),
            ("custom.zxc", Some("zxc")),
            ("archive.tar.gz", Some("gz")),
            ("photo.CR3", Some("CR3")),
            ("README", None),
            ("file.", None),
            (".gitignore", None),
            (".config.json", Some("json")),
            ("..", None),
            ("a..txt", Some("txt")),
            ("日本語.拡張", Some("拡張")),
        ] {
            assert_eq!(extension(name), expected, "{name}");
        }
    }
    #[test]
    fn clipboard_selection() {
        assert!(copy_path(&[]).is_none());
        assert!(copy_path(&["relative.txt".into()]).is_none());
        assert!(copy_path(&["C:\\a".into(), "C:\\b".into()]).is_none());
    }
    #[cfg(windows)]
    #[test]
    fn windows_clipboard_roots_unc_and_unicode() {
        for (input, expected) in [
            (r"C:\", r"C:\"),
            (r"C:\folder\", r"C:\folder"),
            (r"\\server\share\", r"\\server\share\"),
            (r"\\server\share\folder\", r"\\server\share\folder"),
            (r"C:\space dir\日本語.奇妙", r"C:\space dir\日本語.奇妙"),
            (
                r"\\server\share\space dir\é.txt",
                r"\\server\share\space dir\é.txt",
            ),
        ] {
            assert_eq!(copy_path(&[input.into()]).as_deref(), Some(expected));
        }
    }
}
