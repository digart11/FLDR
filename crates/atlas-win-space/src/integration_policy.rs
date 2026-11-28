//! Win+E ownership policy, shared by the Win32 adapter and an in-memory test registry.
use super::{command, Value, DELEGATE};
use std::{io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Slot {
    Target,
    Current,
    Legacy,
}

pub(super) trait Registry {
    fn get(&self, slot: Slot, name: &str) -> io::Result<Option<Value>>;
    fn set(&mut self, slot: Slot, name: &str, value: Option<&Value>) -> io::Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Backup {
    owner: Value,
    command: Option<Value>,
    delegate: Option<Value>,
}

fn conflict() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Win+E has an unknown, changed, or ambiguous owner; its handler and backups were preserved",
    )
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Incomplete or invalid Win+E backup; no changes made",
    )
}
fn owner_name(slot: Slot) -> &'static str {
    if slot == Slot::Legacy {
        "AtlasCommand"
    } else {
        "FLDRCommand"
    }
}
fn pack(value: Option<&Value>) -> Value {
    let mut bytes = Vec::new();
    if let Some(value) = value {
        bytes.extend_from_slice(&value.kind.to_le_bytes());
        bytes.extend_from_slice(&value.bytes);
    }
    Value { kind: 3, bytes } // REG_BINARY; preserve the original type and every byte.
}
fn unpack(value: Value) -> io::Result<Option<Value>> {
    if value.kind != 3 {
        return Err(invalid());
    }
    if value.bytes.is_empty() {
        return Ok(None);
    }
    if value.bytes.len() < 4 {
        return Err(invalid());
    }
    Ok(Some(Value {
        kind: u32::from_le_bytes(value.bytes[..4].try_into().unwrap()),
        bytes: value.bytes[4..].to_vec(),
    }))
}
fn load(registry: &impl Registry, slot: Slot) -> io::Result<Option<Backup>> {
    let owner = registry.get(slot, owner_name(slot))?;
    let command = registry.get(slot, "Command")?;
    let delegate = registry.get(slot, "Delegate")?;
    match (owner, command, delegate) {
        (None, None, None) => Ok(None),
        (Some(owner), Some(command), Some(delegate)) => {
            // Ownership certificates must describe a quoted absolute executable,
            // not arbitrary registry bytes or an empty default value.
            let text = owner.text_value().ok_or_else(invalid)?;
            let path = text
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or_else(invalid)?;
            if super::command(Path::new(path))? != owner {
                return Err(invalid());
            }
            Ok(Some(Backup {
                owner,
                command: unpack(command)?,
                delegate: unpack(delegate)?,
            }))
        }
        _ => Err(invalid()),
    }
}
fn target(registry: &impl Registry) -> io::Result<(Option<Value>, Option<Value>)> {
    Ok((
        registry.get(Slot::Target, "")?,
        registry.get(Slot::Target, DELEGATE)?,
    ))
}
fn owned(values: &(Option<Value>, Option<Value>), backup: &Backup) -> bool {
    values.0.as_ref() == Some(&backup.owner) && values.1 == Some(Value::text(""))
}
fn active(registry: &impl Registry) -> io::Result<Option<(Slot, Backup)>> {
    let current = load(registry, Slot::Current)?;
    let legacy = load(registry, Slot::Legacy)?;
    // Conflicting certificates must never be resolved by guessing which backup won.
    if let (Some(current), Some(legacy)) = (&current, &legacy) {
        if current != legacy {
            return Err(conflict());
        }
    }
    Ok(current
        .map(|b| (Slot::Current, b))
        .or_else(|| legacy.map(|b| (Slot::Legacy, b))))
}
pub(super) fn enabled(registry: &impl Registry) -> io::Result<bool> {
    let backup = active(registry)?;
    let values = target(registry)?;
    Ok(backup.is_some_and(|(_, backup)| owned(&values, &backup)))
}

// Only the absent per-user override and the standard Windows delegate are
// eligible for first registration. Arbitrary commands (even explorer-like ones)
// need an existing, exact ownership certificate.
fn native(values: &(Option<Value>, Option<Value>)) -> bool {
    let empty = |v: &Option<Value>| v.is_none() || v.as_ref() == Some(&Value::text(""));
    empty(&values.0)
        && (empty(&values.1)
            || values.1 == Some(Value::text("{11dbb47c-a525-400b-9e80-a54615a090c0}")))
}

struct Change {
    slot: Slot,
    name: &'static str,
    before: Option<Value>,
    after: Option<Value>,
}
fn plan(
    registry: &impl Registry,
    changes: &mut Vec<Change>,
    slot: Slot,
    name: &'static str,
    after: Option<Value>,
) -> io::Result<()> {
    changes.push(Change {
        slot,
        name,
        before: registry.get(slot, name)?,
        after,
    });
    Ok(())
}
fn apply(registry: &mut impl Registry, changes: &[Change]) -> io::Result<()> {
    // Recheck the entire read set before writing. Never discard a backup on a
    // failed target rollback: stop rollback immediately if any undo fails.
    for change in changes {
        if registry.get(change.slot, change.name)? != change.before {
            return Err(conflict());
        }
    }
    let mut expected_target = target(registry)?;
    for (index, change) in changes.iter().enumerate() {
        // A takeover between writes must not be hidden by our next write or
        // rollback. Keep the recovery certificate if another writer intervenes.
        if target(registry)? != expected_target
            || registry.get(change.slot, change.name)? != change.before
        {
            return Err(conflict());
        }
        if let Err(error) = registry.set(change.slot, change.name, change.after.as_ref()) {
            for undo in changes[..index].iter().rev() {
                if target(registry)? != expected_target {
                    return Err(conflict());
                }
                if let Err(rollback) = registry.set(undo.slot, undo.name, undo.before.as_ref()) {
                    return Err(io::Error::other(format!("Win+E update failed: {error}; rollback failed: {rollback}; recovery metadata retained")));
                }
                if undo.slot == Slot::Target {
                    if undo.name.is_empty() {
                        expected_target.0 = undo.before.clone();
                    } else {
                        expected_target.1 = undo.before.clone();
                    }
                }
            }
            return Err(error);
        }
        if change.slot == Slot::Target {
            if change.name.is_empty() {
                expected_target.0 = change.after.clone();
            } else {
                expected_target.1 = change.after.clone();
            }
        }
    }
    Ok(())
}

pub(super) fn set_enabled(
    registry: &mut impl Registry,
    executable: &Path,
    enabled: bool,
) -> io::Result<()> {
    let registration = active(registry)?;
    let values = target(registry)?;
    if let Some((_, backup)) = &registration {
        if !owned(&values, backup) {
            return Err(conflict());
        }
    } else if !enabled {
        return Ok(());
    } else if !native(&values) {
        return Err(conflict());
    }
    let mut changes = Vec::new();
    if enabled {
        let new_command = command(executable)?;
        let (original_command, original_delegate) = registration
            .as_ref()
            .map_or((values.0.clone(), values.1.clone()), |(_, b)| {
                (b.command.clone(), b.delegate.clone())
            });
        // Originals are durable before either target value changes.
        plan(
            registry,
            &mut changes,
            Slot::Current,
            "Command",
            Some(pack(original_command.as_ref())),
        )?;
        plan(
            registry,
            &mut changes,
            Slot::Current,
            "Delegate",
            Some(pack(original_delegate.as_ref())),
        )?;
        plan(
            registry,
            &mut changes,
            Slot::Current,
            "FLDRCommand",
            Some(new_command.clone()),
        )?;
        plan(registry, &mut changes, Slot::Target, "", Some(new_command))?;
        plan(
            registry,
            &mut changes,
            Slot::Target,
            DELEGATE,
            Some(Value::text("")),
        )?;
    } else if let Some((_, backup)) = &registration {
        plan(
            registry,
            &mut changes,
            Slot::Target,
            "",
            backup.command.clone(),
        )?;
        plan(
            registry,
            &mut changes,
            Slot::Target,
            DELEGATE,
            backup.delegate.clone(),
        )?;
    }
    for slot in [Slot::Legacy, Slot::Current] {
        if (slot == Slot::Legacy && load(registry, slot)?.is_some())
            || (slot == Slot::Current && !enabled)
        {
            // Delete only our three values, after target success. Unknown values
            // and subkeys in either metadata namespace are never deleted.
            for name in [owner_name(slot), "Command", "Delegate"] {
                plan(registry, &mut changes, slot, name, None)?;
            }
        }
    }
    apply(registry, &changes)
}

pub(super) fn refresh(registry: &mut impl Registry, executable: &Path) -> io::Result<bool> {
    if executable
        .file_name()
        .is_none_or(|n| !n.to_string_lossy().eq_ignore_ascii_case("FLDR.exe"))
    {
        return Ok(false);
    }
    let Some((slot, backup)) = active(registry)? else {
        return Ok(false);
    };
    if !owned(&target(registry)?, &backup) {
        return Ok(false);
    }
    let current = command(executable)?;
    // Namespace-only migration is safe for the already-current executable too.
    let same_directory = ["FileOP.exe", "atlas.exe"]
        .iter()
        .any(|name| command(&executable.with_file_name(name)).is_ok_and(|v| v == backup.owner));
    if same_directory || (slot == Slot::Legacy && backup.owner == current) {
        set_enabled(registry, executable, true)?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    const EXE: &str = r"C:\FLDR Test\FLDR.exe";
    #[derive(Default)]
    struct Memory {
        values: BTreeMap<(Slot, String), Value>,
        writes: usize,
        fail_at: Option<usize>,
        takeover_after: Option<usize>,
    }
    impl Registry for Memory {
        fn get(&self, slot: Slot, name: &str) -> io::Result<Option<Value>> {
            Ok(self.values.get(&(slot, name.to_owned())).cloned())
        }
        fn set(&mut self, slot: Slot, name: &str, value: Option<&Value>) -> io::Result<()> {
            self.writes += 1;
            if self.fail_at == Some(self.writes) {
                return Err(io::Error::other("injected write failure"));
            }
            let key = (slot, name.to_owned());
            if let Some(value) = value {
                self.values.insert(key, value.clone());
            } else {
                self.values.remove(&key);
            }
            if self.takeover_after == Some(self.writes) {
                self.values
                    .insert((Slot::Target, String::new()), Value::text("other owner"));
            }
            Ok(())
        }
    }
    fn legacy(registry: &mut Memory, exe: &str, command: Option<Value>, delegate: Option<Value>) {
        let owner = super::command(Path::new(exe)).unwrap();
        registry
            .set(Slot::Legacy, "Command", Some(&pack(command.as_ref())))
            .unwrap();
        registry
            .set(Slot::Legacy, "Delegate", Some(&pack(delegate.as_ref())))
            .unwrap();
        registry
            .set(Slot::Legacy, "AtlasCommand", Some(&owner))
            .unwrap();
        registry.set(Slot::Target, "", Some(&owner)).unwrap();
        registry
            .set(Slot::Target, DELEGATE, Some(&Value::text("")))
            .unwrap();
    }
    fn enable(registry: &mut Memory, value: bool) -> io::Result<()> {
        set_enabled(registry, Path::new(EXE), value)
    }
    #[test]
    fn clean_enable_repeat_and_disable_restore_absence() {
        let mut registry = Memory::default();
        assert!(!enabled(&registry).unwrap());
        enable(&mut registry, false).unwrap();
        assert_eq!(registry.writes, 0);
        enable(&mut registry, true).unwrap();
        enable(&mut registry, true).unwrap();
        assert!(enabled(&registry).unwrap());
        assert!(registry
            .get(Slot::Current, "FLDRCommand")
            .unwrap()
            .is_some());
        assert!(registry
            .get(Slot::Current, "AtlasCommand")
            .unwrap()
            .is_none());
        enable(&mut registry, false).unwrap();
        assert!(registry.values.is_empty());
        enable(&mut registry, true).unwrap();
        enable(&mut registry, false).unwrap();
        assert!(registry.values.is_empty());
    }
    #[test]
    fn standard_windows_delegate_restored_exactly() {
        let mut registry = Memory::default();
        registry
            .set(Slot::Target, "", Some(&Value::text("")))
            .unwrap();
        registry
            .set(
                Slot::Target,
                DELEGATE,
                Some(&Value::text("{11dbb47c-a525-400b-9e80-a54615a090c0}")),
            )
            .unwrap();
        let original = registry.values.clone();
        enable(&mut registry, true).unwrap();
        enable(&mut registry, false).unwrap();
        assert_eq!(registry.values, original);
    }
    #[test]
    fn unknown_first_handler_or_delegate_is_never_overwritten() {
        for (name, value) in [
            ("", Value::text("other.exe")),
            (DELEGATE, Value::text("{unknown}")),
            ("", Value::text(r"C:\Windows\explorer.exe")),
        ] {
            let mut registry = Memory::default();
            registry.set(Slot::Target, name, Some(&value)).unwrap();
            let before = registry.values.clone();
            assert!(enable(&mut registry, true).is_err());
            enable(&mut registry, false).unwrap();
            assert_eq!(registry.values, before);
        }
    }
    #[test]
    fn takeover_preserves_both_handler_and_backup_on_enable_and_disable() {
        for name in ["", DELEGATE] {
            let mut registry = Memory::default();
            enable(&mut registry, true).unwrap();
            registry
                .set(Slot::Target, name, Some(&Value::text("other owner")))
                .unwrap();
            let before = registry.values.clone();
            assert!(!enabled(&registry).unwrap());
            assert!(enable(&mut registry, false).is_err());
            assert!(enable(&mut registry, true).is_err());
            assert!(!refresh(&mut registry, Path::new(EXE)).unwrap());
            assert_eq!(registry.values, before);
        }
    }
    #[test]
    fn legacy_handoff_retains_raw_originals_and_unknown_metadata() {
        let mut registry = Memory::default();
        let original_command = Value {
            kind: 2,
            bytes: vec![0, 255, 42, 0, 0],
        };
        let original_delegate = Value {
            kind: 4,
            bytes: 0x12345678u32.to_le_bytes().to_vec(),
        };
        legacy(
            &mut registry,
            r"C:\FLDR Test\atlas.exe",
            Some(original_command.clone()),
            Some(original_delegate.clone()),
        );
        registry
            .set(Slot::Legacy, "Unknown", Some(&Value::text("keep legacy")))
            .unwrap();
        registry
            .set(Slot::Current, "Unknown", Some(&Value::text("keep current")))
            .unwrap();
        assert!(enabled(&registry).unwrap());
        assert!(!refresh(&mut registry, Path::new(r"C:\Other\FLDR.exe")).unwrap());
        assert!(refresh(&mut registry, Path::new(EXE)).unwrap());
        assert!(!refresh(&mut registry, Path::new(EXE)).unwrap());
        assert!(registry
            .get(Slot::Legacy, "AtlasCommand")
            .unwrap()
            .is_none());
        enable(&mut registry, false).unwrap();
        assert_eq!(
            registry.get(Slot::Target, "").unwrap(),
            Some(original_command)
        );
        assert_eq!(
            registry.get(Slot::Target, DELEGATE).unwrap(),
            Some(original_delegate)
        );
        assert_eq!(registry.values.len(), 4); // two originals and two unknown values
    }
    #[test]
    fn legacy_disable_restores_directly_and_explicit_rebind_preserves_original() {
        for rebind in [false, true] {
            let mut registry = Memory::default();
            let original = Value::text("original Windows value");
            legacy(
                &mut registry,
                r"C:\Old\FileOP.exe",
                Some(original.clone()),
                None,
            );
            if rebind {
                enable(&mut registry, true).unwrap();
            }
            enable(&mut registry, false).unwrap();
            assert_eq!(registry.get(Slot::Target, "").unwrap(), Some(original));
            assert_eq!(registry.values.len(), 1);
        }
    }
    #[test]
    fn already_renamed_legacy_command_migrates_namespace() {
        let mut registry = Memory::default();
        legacy(&mut registry, EXE, None, None);
        assert!(refresh(&mut registry, Path::new(EXE)).unwrap());
        assert!(enabled(&registry).unwrap());
        enable(&mut registry, false).unwrap();
        assert!(registry.values.is_empty());
    }
    #[test]
    fn stale_legacy_registration_is_preserved() {
        let mut registry = Memory::default();
        legacy(&mut registry, r"C:\Old\atlas.exe", None, None);
        registry
            .set(Slot::Target, "", Some(&Value::text("new handler")))
            .unwrap();
        let before = registry.values.clone();
        assert!(!refresh(&mut registry, Path::new(EXE)).unwrap());
        assert!(enable(&mut registry, true).is_err());
        assert!(enable(&mut registry, false).is_err());
        assert_eq!(registry.values, before);
    }
    #[test]
    fn partial_or_malformed_backups_are_never_replaced() {
        for slot in [Slot::Current, Slot::Legacy] {
            for name in [owner_name(slot), "Command", "Delegate"] {
                let mut registry = Memory::default();
                registry
                    .set(slot, name, Some(&Value::text("unexpected")))
                    .unwrap();
                let before = registry.values.clone();
                assert!(enable(&mut registry, true).is_err());
                assert!(enable(&mut registry, false).is_err());
                assert_eq!(registry.values, before);
            }
        }
        assert!(unpack(Value {
            kind: 1,
            bytes: vec![]
        })
        .is_err());
        assert!(unpack(Value {
            kind: 3,
            bytes: vec![0, 1, 2]
        })
        .is_err());
    }
    #[test]
    fn conflicting_dual_certificates_are_preserved() {
        let mut registry = Memory::default();
        enable(&mut registry, true).unwrap();
        legacy(
            &mut registry,
            EXE,
            Some(Value::text("different original")),
            None,
        );
        let before = registry.values.clone();
        assert!(enable(&mut registry, true).is_err());
        assert!(enable(&mut registry, false).is_err());
        assert_eq!(registry.values, before);
    }
    #[test]
    fn equivalent_dual_certificates_can_be_restored() {
        let mut registry = Memory::default();
        enable(&mut registry, true).unwrap();
        legacy(&mut registry, EXE, None, None);
        enable(&mut registry, false).unwrap();
        assert!(registry.values.is_empty());
    }
    #[test]
    fn every_write_failure_rolls_back_registration_handoff_and_restore() {
        for scenario in 0..4 {
            let mut baseline = Memory::default();
            match scenario {
                1 => legacy(
                    &mut baseline,
                    r"C:\FLDR Test\atlas.exe",
                    Some(Value {
                        kind: 2,
                        bytes: vec![42],
                    }),
                    None,
                ),
                2 => enable(&mut baseline, true).unwrap(),
                3 => legacy(
                    &mut baseline,
                    EXE,
                    None,
                    Some(Value {
                        kind: 7,
                        bytes: vec![1, 2, 3, 4],
                    }),
                ),
                _ => (),
            }
            let enabling = scenario < 2;
            let mut success = Memory {
                values: baseline.values.clone(),
                ..Memory::default()
            };
            enable(&mut success, enabling).unwrap();
            for failure in 1..=success.writes {
                let mut registry = Memory {
                    values: baseline.values.clone(),
                    fail_at: Some(failure),
                    ..Memory::default()
                };
                assert!(enable(&mut registry, enabling).is_err());
                assert_eq!(
                    registry.values, baseline.values,
                    "scenario {scenario}, failure {failure}"
                );
            }
        }
    }
    #[test]
    fn intervening_takeover_keeps_the_new_owner_and_original_backup() {
        for takeover_after in [3, 4] {
            let mut registry = Memory {
                takeover_after: Some(takeover_after),
                ..Memory::default()
            };
            assert!(enable(&mut registry, true).is_err());
            assert_eq!(
                registry.get(Slot::Target, "").unwrap(),
                Some(Value::text("other owner"))
            );
            assert_eq!(
                registry.get(Slot::Current, "Command").unwrap(),
                Some(pack(None))
            );
            assert_eq!(
                registry.get(Slot::Current, "Delegate").unwrap(),
                Some(pack(None))
            );
            assert!(enable(&mut registry, false).is_err());
        }
    }
    #[test]
    fn executable_validation_rejects_relative_quoted_and_nul_paths() {
        for exe in ["FLDR.exe", "C:\\bad\"\\FLDR.exe", "C:\\bad\0\\FLDR.exe"] {
            let mut registry = Memory::default();
            assert!(set_enabled(&mut registry, Path::new(exe), true).is_err());
            assert_eq!(registry.writes, 0);
        }
    }
}
