//! Pure Grid presentation. Cache reads never enqueue scan requests.
use atlas_model::{file_entry::FileKind, folder_stats::SizeCell, settings::ViewMode};
pub(crate) fn parent_entry_visible(mode: &ViewMode) -> bool { *mode != ViewMode::Grid }
pub(crate) fn folder_label(kind: &FileKind, shortcut: bool, side: f32, zoom: f32, cell: SizeCell) -> Option<String> {
    if *kind != FileKind::Dir || shortcut || !zoom.is_finite() || zoom <= 0.0 || !side.is_finite() || side < 64.0 * zoom { return None; }
    match cell {
        SizeCell::Exact(_) | SizeCell::Cached(_) | SizeCell::Partial(_) => Some(super::folder_stats_presentation::text(cell)),
        _ => None,
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn synthetic_parent_has_no_grid_row_including_roots_and_sorted_views() {
        assert!(!parent_entry_visible(&ViewMode::Grid));
        assert!(parent_entry_visible(&ViewMode::Details));assert!(parent_entry_visible(&ViewMode::List));
    }
    #[test] fn labels_require_real_folder_readable_icon_and_existing_values() {
        assert_eq!(folder_label(&FileKind::Dir,false,64.,1.,SizeCell::Cached(42)).unwrap(),"42 B");
        assert!(folder_label(&FileKind::Dir,false,64.,1.,SizeCell::Partial(42)).is_some());
        assert!(folder_label(&FileKind::Dir,false,64.,1.,SizeCell::Exact(0)).is_some());
        for state in [SizeCell::Unknown,SizeCell::Calculating,SizeCell::Stale,SizeCell::Failed,SizeCell::Cancelled] { assert!(folder_label(&FileKind::Dir,false,64.,1.,state).is_none()); }
        for zoom in [1.,1.25,1.5,2.] { assert!(folder_label(&FileKind::Dir,false,55.*zoom,zoom,SizeCell::Exact(1)).is_none());assert!(folder_label(&FileKind::Dir,false,64.*zoom,zoom,SizeCell::Exact(1)).is_some()); }
        assert!(folder_label(&FileKind::Dir,true,64.,1.,SizeCell::Exact(1)).is_none());
        assert!(folder_label(&FileKind::File,false,64.,1.,SizeCell::Exact(1)).is_none());
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BadgeMetrics { pub font: f32, pub height: f32, pub width: f32, pub padding: f32, pub offset: f32 }
pub(crate) fn badge_metrics(side: f32, zoom: f32) -> Option<BadgeMetrics> {
    if !side.is_finite() || !zoom.is_finite() || zoom <= 0.0 || side < 64.0*zoom {return None;}
    // Verified reference: 10px at UI zoom; increase by 25%, not by icon area.
    let font=12.5*zoom;
    Some(BadgeMetrics { font, height: 19.0*zoom, width: side*0.80, padding: 3.0*zoom, offset: side*0.10 })
}
