//! Dependency-free build identity, shared by the model build and its tests.
use std::{path::{Path, PathBuf}, process::Command};

pub fn utc_date(seconds: u64) -> String {
    let days = (seconds / 86400) as i64 + 719468;
    let era = days / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC", seconds / 3600 % 24, seconds / 60 % 60, seconds % 60)
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").env("GIT_OPTIONAL_LOCKS", "0").current_dir(root).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn collect(root: &Path, path: &Path, files: &mut Vec<PathBuf>) {
    if path.to_string_lossy().to_lowercase().contains("conflicted copy")
        || std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) { return; }
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() { collect(root, &entry.path(), files); }
        }
    } else if let Some(name) = path.to_str() {
        if !name.to_lowercase().contains("conflicted copy") && path.strip_prefix(root).is_ok()
            && matches!(path.extension().and_then(|v| v.to_str()), Some("rs" | "cs" | "toml" | "lock" | "yml" | "rc" | "ico" | "png" | "ps1" | "iss" | "appxmanifest")) {
            files.push(path.to_path_buf());
        }
    }
}

pub fn fingerprint(root: &Path) -> String {
    let mut files = Vec::new();
    for path in ["src", "crates", "scripts", "packaging", "Cargo.toml", "Cargo.lock", "FLDR-icon.ico", "FLDR-logo.png"] {
        collect(root, &root.join(path), &mut files);
    }
    files.sort();
    let mut hash = 0xcbf29ce484222325u64;
    for file in files {
        let name = file.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        for byte in name.bytes().chain(std::fs::read(&file).unwrap_or_default()) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:016x}")
}

pub fn generate(root: &Path) -> String {
    let revision = git(root, &["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "no-git".into());
    let state = git(root, &["status", "--porcelain", "--untracked-files=normal"])
        .map_or("unknown", |s| if s.is_empty() { "clean" } else { "dirty" });
    let timestamp = std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|s| s.parse::<u64>().ok())
        .filter(|seconds| *seconds <= 253402300799)
        .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
    let date = utc_date(timestamp);
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    let source = fingerprint(root);
    format!("pub const REVISION: &str = {revision:?};\npub const TREE_STATE: &str = {state:?};\npub const BUILD_DATE: &str = {date:?};\npub const BUILD_PROFILE: &str = {profile:?};\npub const BUILD_TARGET: &str = {target:?};\npub const SOURCE_ID: &str = {source:?};\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_are_real_utc_dates() {
        assert_eq!(super::utc_date(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(super::utc_date(1700000000), "2023-11-14 22:13:20 UTC");
        assert_eq!(super::utc_date(951782400), "2000-02-29 00:00:00 UTC");
    }
}
