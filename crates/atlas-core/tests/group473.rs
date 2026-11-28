//! Recovery guards: source wiring plus pure presentation policies, not GUI claims.
#[path = "../../atlas-ui/src/ui/status_layout.rs"] mod status_layout;
#[path = "../../atlas-ui/src/ui/glass_ink.rs"] mod glass_ink;
#[path = "../../atlas-ui/src/ui/grid_presentation.rs"] mod grid_presentation;
#[path = "../../atlas-ui/src/ui/folder_stats_presentation.rs"] mod folder_stats_presentation;
#[path = "../../atlas-ui/src/ui/tab_shape.rs"] mod tab_shape;

#[test]
fn bottom_information_is_absolute_and_unconditional_in_each_visible_pane() {
    let chrome = include_str!("../../atlas-ui/src/ui/chrome.rs");
    let render = include_str!("../../atlas-ui/src/ui/dock/render.rs").replace("\r\n", "\n");
    let start = chrome.find("let status_pill: AnyElement").unwrap();
    let end = chrome[start..].find("// ── Right pill:").unwrap() + start;
    let info = &chrome[start..end];
    assert!(info.contains("let status_pill = h_flex().absolute()"));
    assert!(!info.contains("relative().size_full()"));
    assert!(info.find(".child(status_pill)").unwrap() < info.find(".children(metadata.filter").unwrap());
    assert!(info.contains(".children(extension.map"));
    assert!(info.contains(".truncate().child(compact)"));
    assert!(info.contains("Action::ToggleInspector"));
    assert!(info.contains("if !focused { return (filter_pill,"));
    let pane = &render[render.find("let mut pane_box = div().size_full().relative().child(content);").unwrap()..];
    assert!(pane.starts_with("let mut pane_box = div().size_full().relative().child(content);\n            // Every visible pane"));
    assert!(!pane[..pane.find("render_folder_size_notice").unwrap()].contains("if is_focused"));
    // Metadata arrives, disappears, or belongs to an obsolete selection: none of
    // those values is accepted by the anchor policy, so the anchor stays fixed.
    for width in [280., 480., 640., 900., 1600.] {
        for focused in [false, true] {
            let a = status_layout::placement(width, focused);
            assert!(width - 8. - a.right > 100.);
            if focused && width < 640. { assert!(a.bottom >= 50.); }
            else { assert_eq!(a.bottom, 8.); }
        }
    }
}

#[test]
fn moderate_badges_stay_in_lower_folder_interior_at_every_scale() {
    for zoom in [0.75, 1., 1.25, 1.5, 2., 3.] {
        for side in [64., 72., 96., 128., 256.] {
            let m = grid_presentation::badge_metrics(side*zoom, zoom).unwrap();
            assert_eq!(m.font, 10.*1.25*zoom);
            assert!(m.width <= side*zoom*0.88);
            assert!(m.offset + m.height < side*zoom*0.5);
            assert!(m.height - 4.*zoom >= m.font);
        }
    }
    for (side, zoom) in [(f32::NAN,1.), (64.,0.), (64.,f32::INFINITY)] {
        assert!(grid_presentation::badge_metrics(side,zoom).is_none());
    }
}

#[test]
fn glass_secondary_improves_light_and_dark_backings_preserving_hierarchy() {
    let linear = |v: f32| if v <= 0.04045 { v/12.92 } else { ((v+0.055)/1.055).powf(2.4) };
    let contrast = |a: f32,b: f32| (linear(a).max(linear(b))+0.05)/(linear(a).min(linear(b))+0.05);
    // Representative canvas composites at low density over dark/white wallpapers.
    for (primary, muted, backings) in [(0.95,0.45,[0.02,0.30,0.57]), (0.10,0.50,[0.32,0.70,0.95])] {
        let ink = glass_ink::secondary([muted;3],[primary;3])[0];
        for background in backings {
            assert!(contrast(ink,background) > contrast(muted,background));
            assert!(contrast(primary,background) > contrast(ink,background));
            let disabled = ink*0.45 + background*0.55;
            assert!(contrast(ink,background) > contrast(disabled,background));
        }
    }
    let colored = glass_ink::secondary([0.4,0.3,0.5],[0.9,0.85,0.8]);
    assert_ne!(colored[0], colored[1]); // preserve theme hue; never hardcode white
}

#[test]
fn active_outline_is_removed_and_open_geometry_is_preserved() {
    let source = include_str!("../../atlas-ui/src/ui/views/tabs_bar_element.rs");
    assert!(!source.contains("PathBuilder::stroke"));
    assert!(!source.contains("edge.close"));
    let poly = tab_shape::polygon(0.,4.,160.,36.,5.);
    let edges = tab_shape::visible_edges(&poly,&[]);
    assert_eq!(edges.len(),3);
    assert_eq!(edges[0].1,edges[1].0);
    assert_eq!(edges[1].1,edges[2].0);
    assert!(edges.iter().all(|(a,b)| a.1 != 40. || b.1 != 40.));
}
