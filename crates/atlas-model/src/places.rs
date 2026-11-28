//! Sidebar references only. No filesystem access, including during normalization.
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlaceTarget {
    Folder(PathBuf),
    RecycleBin,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Place {
    pub label: String,
    pub target: PlaceTarget,
    pub icon: String,
    #[serde(default)]
    pub hidden: bool,
}

impl Place {
    pub fn key(&self) -> Option<String> {
        match &self.target {
            PlaceTarget::Folder(path) => normalized_path(&path.to_string_lossy()),
            PlaceTarget::RecycleBin => Some("recycle-bin".into()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Places {
    /// None uses defaults; Some(empty) intentionally has no references.
    pub entries: Option<Vec<Place>>,
}

pub struct PlacesFeature;
impl crate::profiles::ProfileScoped for PlacesFeature {
    const KEY: &'static str = "fileop.places";
    type State = Places;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NetworkLocation {
    pub name: String,
    pub path: PathBuf,
}

impl NetworkLocation {
    pub fn key(&self) -> Option<String> {
        crate::location::network_server_root(&self.path)
            .map(|server| format!(r"\\{}", server.to_ascii_lowercase()))
            .or_else(|| normalized_path(&self.path.to_string_lossy()))
    }
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SidebarPreferences {
    pub visible_legacy_sections: Vec<crate::profiles::SectionKind>,
    pub hide_file_dock: bool,
    pub hidden_provider_folders: std::collections::BTreeSet<String>,
    pub network_locations: Vec<NetworkLocation>,
    pub hidden_network_shortcuts: std::collections::BTreeSet<String>,
}

pub struct SidebarFeature;
impl crate::profiles::ProfileScoped for SidebarFeature {
    const KEY: &'static str = "fldr.sidebar";
    type State = SidebarPreferences;
}

impl Places {
    pub fn resolved(&self, defaults: Vec<Place>) -> Vec<Place> {
        let mut seen = std::collections::HashSet::new();
        self.entries
            .as_ref()
            .unwrap_or(&defaults)
            .iter()
            .filter(|entry| entry.key().is_some_and(|key| seen.insert(key)))
            .cloned()
            .collect()
    }

    pub fn add(&mut self, defaults: Vec<Place>, place: Place) -> bool {
        let Some(key) = place.key() else {
            return false;
        };
        let mut entries = self.resolved(defaults);
        if entries
            .iter()
            .any(|entry| entry.key().as_ref() == Some(&key))
        {
            return false;
        }
        entries.push(place);
        self.entries = Some(entries);
        true
    }

    pub fn edit(&mut self, defaults: Vec<Place>, key: &str, edit: PlaceEdit) {
        let mut entries = self.resolved(defaults);
        if let Some(index) = entries
            .iter()
            .position(|entry| entry.key().as_deref() == Some(key))
        {
            match edit {
                PlaceEdit::Remove => {
                    entries.remove(index);
                }
                PlaceEdit::ToggleHidden => entries[index].hidden = !entries[index].hidden,
                PlaceEdit::MoveUp if index > 0 => entries.swap(index, index - 1),
                PlaceEdit::MoveDown if index + 1 < entries.len() => entries.swap(index, index + 1),
                PlaceEdit::MoveUp | PlaceEdit::MoveDown => {}
            }
            self.entries = Some(entries);
        }
    }

    pub fn restore(&mut self, confirmed: bool) {
        if confirmed {
            self.entries = None;
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PlaceEdit {
    Remove,
    ToggleHidden,
    MoveUp,
    MoveDown,
}

/// Lexical Windows identity; never canonicalize/stat a potentially offline share.
pub fn normalized_path(path: &str) -> Option<String> {
    let path = path.replace('/', "\\");
    let path = if path
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("\\\\?\\UNC\\"))
    {
        format!("\\\\{}", &path[8..])
    } else {
        path.strip_prefix("\\\\?\\").unwrap_or(&path).to_string()
    };
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '<' | '>' | '|' | '*' | '?'))
    {
        return None;
    }
    let (root, tail) = if let Some(unc) = path.strip_prefix("\\\\") {
        let mut parts = unc.split('\\').filter(|p| !p.is_empty());
        let server = parts.next()?;
        let share = parts.next()?;
        if [server, share]
            .iter()
            .any(|p| matches!(*p, "." | "..") || p.contains(':'))
        {
            return None;
        }
        (
            format!("\\\\{server}\\{share}"),
            parts.collect::<Vec<_>>().join("\\"),
        )
    } else if path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic() {
        if path.len() > 2 && path.as_bytes().get(2) != Some(&b'\\') {
            return None;
        }
        (
            path[..2].to_string(),
            path.get(3..).unwrap_or("").to_string(),
        )
    } else {
        return None;
    };
    let mut parts = Vec::new();
    for part in tail.split('\\').filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            parts.pop();
        } else {
            if part.contains(':') || part.ends_with([' ', '.']) {
                return None;
            }
            parts.push(part);
        }
    }
    Some(format!("{root}\\{}", parts.join("\\")).to_lowercase())
}

/// Tracks reference ownership independently of the content pane's old selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeleteOwner {
    #[default]
    Content,
    SidebarReference,
}
impl DeleteOwner {
    pub fn allows_filesystem_delete(self) -> bool {
        self == Self::Content
    }

    pub fn blocks_command(self, command: &str) -> bool {
        !self.allows_filesystem_delete()
            && matches!(
                command,
                "files.trash" | "files.delete" | "recycle.empty" | "recycle.purge"
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn place(path: &str) -> Place {
        Place {
            label: path.into(),
            target: PlaceTarget::Folder(path.into()),
            icon: "folder".into(),
            hidden: false,
        }
    }
    #[test]
    fn windows_identity_is_lexical_case_insensitive_and_unc_aware() {
        for (a, b) in [
            (r"C:\Work\A\..\B\", "c:/work/b"),
            ("C:", "c:/"),
            (r"\\server\share\a\..\b", r"\\SERVER\SHARE\b\"),
            (r"\\?\UNC\server\share\b", r"\\server\share\b"),
            (r"\\?\C:\Work", "c:/work"),
        ] {
            assert_eq!(normalized_path(a), normalized_path(b));
            assert!(normalized_path(a).is_some());
        }
        for invalid in [
            "C:relative",
            "relative",
            r"\\server",
            r"\\.\pipe\x",
            r"C:\file:stream",
            r"C:\odd.",
        ] {
            assert!(normalized_path(invalid).is_none(), "{invalid}");
        }
    }
    #[test]
    fn adding_deduplicates_visible_and_hidden_definitions() {
        let mut places = Places::default();
        assert!(places.add(vec![], place(r"C:\Work")));
        let key = normalized_path(r"C:\Work").unwrap();
        places.edit(vec![], &key, PlaceEdit::ToggleHidden);
        assert!(!places.add(vec![], place("c:/WORK/")));
        assert!(places.resolved(vec![])[0].hidden);
    }
    #[test]
    fn persistence_preserves_order_visibility_unc_and_empty_customization() {
        let mut places = Places {
            entries: Some(vec![place(r"C:\a"), place(r"\\offline\share\b")]),
        };
        let key = normalized_path(r"\\offline\share\b").unwrap();
        places.edit(vec![], &key, PlaceEdit::ToggleHidden);
        places.edit(vec![], &key, PlaceEdit::MoveUp);
        let encoded = serde_json::to_string(&places).unwrap();
        let decoded: Places = serde_json::from_str(&encoded).unwrap();
        assert_eq!(places, decoded);
        assert_eq!(decoded.resolved(vec![])[0].key(), Some(key));
        let empty: Places = serde_json::from_str(r#"{"entries":[]}"#).unwrap();
        assert!(empty.resolved(vec![place(r"C:\default")]).is_empty());
    }
    #[test]
    fn reference_removal_and_restore_cannot_touch_files_or_content_selection() {
        let selected_content = vec![PathBuf::from(r"C:\important.txt")];
        let mut places = Places {
            entries: Some(vec![place(r"C:\important")]),
        };
        let owner = DeleteOwner::SidebarReference;
        // Delete and Shift+Delete share this same dispatch policy.
        for _permanent in [false, true] {
            assert!(!owner.allows_filesystem_delete());
        }
        places.edit(
            vec![],
            &normalized_path(r"C:\important").unwrap(),
            PlaceEdit::Remove,
        );
        assert_eq!(selected_content, [PathBuf::from(r"C:\important.txt")]);
        assert_eq!(places.entries, Some(vec![]));
        places.restore(false);
        assert_eq!(places.entries, Some(vec![]));
        places.restore(true);
        assert_eq!(places.entries, None);
        assert!(DeleteOwner::Content.allows_filesystem_delete());
    }
    #[test]
    fn profile_namespace_round_trips_without_changing_other_preferences() {
        let mut profiles = crate::profiles::ProfileManager::default();
        let profile = profiles.active_mut();
        profile.prefs.set("unrelated", &42);
        let places = Places {
            entries: Some(vec![place(r"\\offline\share")]),
        };
        profile.set_scoped::<PlacesFeature>(&places);
        assert_eq!(profile.scoped::<PlacesFeature>(), places);
        assert_eq!(profile.prefs.get::<i32>("unrelated"), Some(42));
        let bytes = serde_json::to_vec(&profiles).unwrap();
        let restored: crate::profiles::ProfileManager = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.active().scoped::<PlacesFeature>(), places);
        assert_eq!(restored.active().prefs.get::<i32>("unrelated"), Some(42));
    }

    #[test]
    fn remapped_delete_and_recycle_commands_cannot_use_stale_content_selection() {
        for command in [
            "files.trash",
            "files.delete",
            "recycle.empty",
            "recycle.purge",
        ] {
            assert!(
                DeleteOwner::SidebarReference.blocks_command(command),
                "{command}"
            );
            assert!(!DeleteOwner::Content.blocks_command(command), "{command}");
        }
        assert!(!DeleteOwner::SidebarReference.blocks_command("nav.refresh"));
    }

    #[test]
    fn order_boundaries_hidden_entries_and_recycle_reference_are_preserved() {
        let mut places = Places {
            entries: Some(vec![
                place(r"C:\a"),
                place(r"C:\b"),
                Place {
                    label: "Recycle Bin".into(),
                    target: PlaceTarget::RecycleBin,
                    icon: "trash".into(),
                    hidden: false,
                },
            ]),
        };
        let first = normalized_path(r"C:\a").unwrap();
        places.edit(vec![], &first, PlaceEdit::MoveUp);
        places.edit(vec![], "recycle-bin", PlaceEdit::ToggleHidden);
        places.edit(vec![], "recycle-bin", PlaceEdit::MoveUp);
        assert!(places.resolved(vec![])[1].hidden);
        places.edit(vec![], "recycle-bin", PlaceEdit::MoveDown);
        places.edit(vec![], "recycle-bin", PlaceEdit::MoveDown);
        assert_eq!(places.resolved(vec![])[2].target, PlaceTarget::RecycleBin);
        places.edit(vec![], "recycle-bin", PlaceEdit::Remove);
        assert_eq!(places.resolved(vec![]).len(), 2);
    }
}
