//! Presentation only: leave legacy sections, bookmarks, tags and preferences saved.
use atlas_model::profiles::{SectionKind, SidebarSection};

pub(crate) fn rank(kind: &SectionKind) -> Option<usize> {
    match kind {
        SectionKind::Places => Some(0),
        SectionKind::Drives => Some(1),
        SectionKind::Cloud => Some(2),
        SectionKind::Recent => Some(3),
        SectionKind::Bookmarks => Some(4),
        SectionKind::Tags => Some(5),
        _ => None,
    }
}

/// Preserve original indices for existing section actions, independently of
/// presentation order. FILE DOCK has its own fixed owner below these sections.
pub(crate) fn sections(saved: &[SidebarSection], visible_legacy: &[SectionKind]) -> Vec<(usize, SidebarSection)> {
    let mut result: Vec<_> = saved.iter().enumerate().filter(|(_, section)| {
        rank(&section.kind).is_some() && (!matches!(section.kind, SectionKind::Bookmarks | SectionKind::Tags)
            || visible_legacy.contains(&section.kind))
    })
        .map(|(index, section)| (index, section.clone())).collect();
    result.sort_by_key(|(_, section)| rank(&section.kind));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expanded_and_collapsed_views_share_the_same_order_and_saved_indices() {
        let saved: Vec<_> = [SectionKind::Tags, SectionKind::Recent, SectionKind::Bookmarks,
            SectionKind::Drives, SectionKind::Places, SectionKind::Cloud, SectionKind::Linux]
            .into_iter().map(SidebarSection::new).collect();
        let result = sections(&saved, &[]);
        assert_eq!(result.iter().map(|(index, _)| *index).collect::<Vec<_>>(), [4, 3, 5, 1]);
        assert_eq!(result.iter().map(|(_, section)| &section.kind).collect::<Vec<_>>(),
            [&SectionKind::Places, &SectionKind::Drives, &SectionKind::Cloud, &SectionKind::Recent]);
    }

    #[test]
    fn presentation_preserves_entire_legacy_profile_and_places_preferences() {
        use atlas_model::{places::PlacesFeature, profiles::{Bookmark, ProfileManager, Tag}};
        let mut profiles = ProfileManager::default();
        let profile = profiles.active_mut();
        profile.bookmarks.push(Bookmark::new("Legacy".into(), r"C:\legacy".into()));
        profile.tags.push(Tag { name: "Legacy Tag".into(), color: [20, 40, 60], global: false });
        let mut places = profile.scoped::<PlacesFeature>();
        places.entries = Some(vec![]);
        profile.set_scoped::<PlacesFeature>(&places);
        profile.sidebar_sections[0].hidden = true;
        let before = serde_json::to_value(&profiles).unwrap();
        let visible = sections(&profiles.active().sidebar_sections, &[]);
        assert!(visible.iter().any(|(_, section)| section.kind == SectionKind::Places && section.hidden));
        assert_eq!(serde_json::to_value(&profiles).unwrap(), before);
        let restored: ProfileManager = serde_json::from_value(before).unwrap();
        assert_eq!(restored.active().bookmarks[0].name, "Legacy");
        assert_eq!(restored.active().tags[0].name, "Legacy Tag");
        assert_eq!(restored.active().scoped::<PlacesFeature>(), places);
    }
}
