//! One Size cell policy for the existing virtualized and fallback Details paths.
use atlas_model::folder_stats::SizeCell;
pub(crate) fn text(cell: SizeCell) -> String {
    match cell {
        SizeCell::Exact(bytes) => {
            atlas_core::util::display::format_size(bytes)
        }
        SizeCell::Cached(bytes) => atlas_core::util::display::format_size(bytes),
        SizeCell::Calculating => "\u{2026}".into(),
        SizeCell::Partial(bytes) => {
            atlas_core::util::display::format_size_state(bytes, false, false)
        }
        SizeCell::Unknown | SizeCell::Failed | SizeCell::Cancelled | SizeCell::Stale => {
            "\u{2014}".into()
        }
    }
}

/// Cached snapshots remain readable but are visually quieter than fresh results.
pub(crate) fn subdued(cell: SizeCell) -> bool {
    matches!(cell, SizeCell::Cached(_))
}

/// Counts share the size state policy, without a byte unit.
pub(crate) fn count(cell: SizeCell) -> String {
    match cell {
        SizeCell::Exact(n) => n.to_string(),
        SizeCell::Cached(n) => n.to_string(),
        SizeCell::Partial(n) => format!("\u{2265} {n}"),
        other => text(other),
    }
}
