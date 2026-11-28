//! Pane-local anchors. Optional metadata never participates in placement decisions.
#[derive(Debug, PartialEq)]
pub(crate) struct Placement { pub bottom: f32, pub right: f32 }
pub(crate) fn placement(width: f32, focused: bool) -> Placement {
    // Narrow panes keep controls at bottom right and information one row above.
    if focused && width < 320.0 { Placement { bottom: 54.0, right: 8.0 } }
    else { Placement { bottom: 8.0, right: if focused { 200.0 } else { 8.0 } } }
}
