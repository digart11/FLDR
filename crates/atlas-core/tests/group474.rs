//! Source integration guards complement the measured Taffy layout regression.
#[test]
fn information_pills_use_matching_regular_text_and_preserve_metadata_overflow() {
    assert!(!atlas_core::util::display::format_size_state(1024, true, false).is_empty());
    let chrome=include_str!("../../atlas-ui/src/ui/chrome.rs");
    let start=chrome.find(".children(metadata.filter").unwrap();
    let end=chrome[start..].find("let filter_pill:").unwrap()+start;
    let metadata=&chrome[start..end];
    assert!(metadata.contains("!compact.trim().is_empty()"));
    assert_eq!(metadata.matches(".flex_initial()").count(),2);
    assert!(!metadata.contains(".flex_1()"));
    assert!(metadata.contains(".text_xs().font_weight(gpui::FontWeight::NORMAL).text_color(controls.foreground)"));
    assert!(metadata.contains("Tooltip::new(expanded.clone())"));
    assert!(metadata.contains(".truncate().child(compact)"));
    assert!(metadata.contains("Action::ToggleInspector"));
    let pills=&chrome[chrome.find("let status_pill: AnyElement").unwrap()..end];
    assert_eq!(pills.matches(".text_xs().font_weight(gpui::FontWeight::NORMAL)").count(),3);
    assert_eq!(pills.matches(".min_h(crate::ui::rem_px(PILL_HEIGHT))").count(),3);
    assert_eq!(pills.matches(".rounded_xl()").count(),3);
    assert_eq!(pills.matches(".px_2()").count(),3);
    assert_eq!(pills.matches(".items_center()").count(),5); // count group and row included
    assert!(!pills.contains(".py")); // all retain zero explicit vertical padding
    let tabs=include_str!("../../atlas-ui/src/ui/views/tabs_bar_element.rs");
    assert!(!tabs.contains("PathBuilder::stroke"));
    assert!(tabs.contains("tab_shape::partition(bar, &silhouettes)"));
}
