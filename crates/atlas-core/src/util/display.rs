#![allow(dead_code)] // reason: retained API surface not exercised by every GPUI flow yet
use std::path::Path;

/// Present legacy preset identifiers with the current product name.
/// Stored IDs and bundled/custom palette contents remain unchanged.
pub fn theme_label(name: &str) -> String {
    name.to_owned()
}

pub fn format_exact_bytes(bytes: u128, separator: char) -> String {
    let digits = bytes.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3 + 6);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 { result.push(separator); }
        result.push(ch);
    }
    result.push_str(" bytes");
    result
}

pub fn clipboard_path_text(paths: &[std::path::PathBuf]) -> String {
    if let Some(text) = atlas_model::file_presentation::copy_path(paths) { return text; }
    paths.iter().map(|path| {
        let text = path.to_string_lossy().replace('\0', "");
        if text.chars().any(char::is_whitespace) { format!("\"{}\"", text.replace('"', "\\\"")) }
        else { text }
    }).collect::<Vec<_>>().join("\r\n")
}
use std::time::SystemTime;

/// Consistent color for folder/directory icons across all views, as neutral RGB.
pub const FOLDER_ICON_COLOR: [u8; 3] = [200, 180, 80];

pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    for (unit, label) in [(TB, "TB"), (GB, "GB"), (MB, "MB"), (KB, "KB")] {
        if bytes >= unit {
            let value = format!("{:.1}", bytes as f64 / unit as f64);
            return format!("{} {label}", value.strip_suffix(".0").unwrap_or(&value));
        }
    }
    format!("{bytes} B")
}

/// A lower bound takes precedence over a cached estimate; byte values stay exact.
pub fn format_size_state(bytes: u64, complete: bool, cached: bool) -> String {
    let prefix = if !complete { "\u{2265}" } else if cached { "~" } else { "" };
    format!("{prefix}{}", format_size(bytes))
}

pub fn format_date(t: SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (y, mo, d, h, mi) = unix_to_ymdhm(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

fn unix_to_ymdhm(secs: u64) -> (u32, u32, u32, u32, u32) {
    // Simple calendar calculation (no external dep)
    let mut days = (secs / 86400) as u32;
    let time_of_day = (secs % 86400) as u32;
    let h = time_of_day / 3600;
    let mi = (time_of_day % 3600) / 60;

    let mut year = 1970u32;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let leap = is_leap(year);
    let month_days: [u32; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u32;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }

    (year, month, days + 1, h, mi)
}

const fn is_leap(y: u32) -> bool {
    y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400))
}

/// Check if a path is hidden (name starts with '.').
pub fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

/// Extract a human-readable display name from a path.
pub fn display_name(path: &Path) -> String {
    if let Some(name) = crate::vault::live::display_name(path) { return name; }
    path.file_name().map_or_else(
        || {
            #[cfg(windows)]
            if let Some(std::path::Component::Prefix(prefix)) = path.components().next() {
                if let Some(label) = unc_share_label(prefix.kind()) {
                    return label;
                }
            }
            path.to_string_lossy().to_string()
        },
        |n| n.to_string_lossy().to_string(),
    )
}

/// A UNC root's useful name is its share (the distro for WSL). Keep the actual
/// path intact for navigation, history and file operations.
#[cfg(windows)]
pub fn unc_share_label(prefix: std::path::Prefix<'_>) -> Option<String> {
    match prefix {
        std::path::Prefix::UNC(_, share) | std::path::Prefix::VerbatimUNC(_, share) =>
            Some(share.to_string_lossy().into_owned()),
        _ => None,
    }
}

#[cfg(all(test, windows))]
mod path_label_tests {
    use super::*;

    #[test]
    fn network_roots_show_the_share_and_children_show_the_folder() {
        for root in [r"\\wsl.localhost\Arch\", r"\\wsl$\Arch", r"\\?\UNC\wsl.localhost\Arch\"] {
            assert_eq!(display_name(Path::new(root)), "Arch");
        }
        assert_eq!(display_name(Path::new(r"\\server\Projects\")), "Projects");
        assert_eq!(display_name(Path::new(r"\\wsl.localhost\Arch\home\me")), "me");
        assert_eq!(display_name(Path::new(r"C:\")), r"C:\");
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;
    #[test]
    fn file_sizes_have_at_most_one_decimal_without_trailing_zero() {
        let gb = 1024_u64.pow(3);
        for (bytes, expected) in [(0, "0 B"), (1023, "1023 B"), (1024, "1 KB"),
            (1536, "1.5 KB"), (4 * gb, "4 GB"), (129 * gb / 10, "12.9 GB"),
            (4 * 1024_u64.pow(4), "4 TB")] {
            assert_eq!(format_size(bytes), expected);
        }
        for bytes in [1024, 1024_u64.pow(2), gb, 1024_u64.pow(4), u64::MAX] {
            let text = format_size(bytes);
            let number = text.split(' ').next().unwrap();
            assert!(number.split_once('.').is_none_or(|(_, fraction)| fraction.len() == 1));
        }
    }
    #[test]
    fn incomplete_and_cached_markers_preserve_the_exact_byte_input() {
        let gb = 1024_u64.pow(3);
        assert_eq!(format_size_state(158 * gb / 10, false, false), "\u{2265}15.8 GB");
        assert_eq!(format_size_state(213 * gb / 10, true, true), "~21.3 GB");
        assert_eq!(format_size_state(4 * gb, false, true), "\u{2265}4 GB");
        assert_eq!(format_size_state(4 * gb, true, false), "4 GB");
        assert_eq!(format_exact_bytes(4 * gb as u128 + 123, ','), "4,294,967,419 bytes");
    }
}
