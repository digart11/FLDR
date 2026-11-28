use crate::app::App;
use crate::ui::{
    icons::{theme_icon, AtlasIcon},
    motion::MotionFeedbackExt as _,
    surface::{line_width, shape_text},
};
use atlas_model::{
    app_state::BgResult,
    folder_compare::{Comparison, Mark},
    pane::TabId,
    tab::LoadState,
};
use gpui::prelude::*;
use gpui::{
    div, point, px, size, AnyElement, Bounds, ColorExt as _, Context, MouseButton, Pixels,
    TextAlign, WeakEntity, Window,
};
use gpui_component::{
    menu::{ContextMenuExt, PopupMenuItem},
    ActiveTheme as _,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[derive(Clone, PartialEq, Eq)]
struct DirectoryVersion {
    pane: TabId,
    path: PathBuf,
    navigation: u64,
    entries: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComparisonUnavailable {
    NeedsTwoFolders,
    BranchView,
}

#[derive(Default)]
pub(crate) struct FolderComparison {
    enabled: bool,
    verify: bool,
    input: Option<[DirectoryVersion; 2]>,
    generation: u64,
    running: Option<(u64, Arc<AtomicBool>)>,
    marks: Option<Comparison>,
    painted_controls: Option<(bool, bool, Result<[TabId; 2], ComparisonUnavailable>, bool, bool)>,
}

impl Drop for FolderComparison {
    fn drop(&mut self) {
        if let Some((_, cancel)) = &self.running {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl App {
    fn comparison_panes(&self) -> Result<[TabId; 2], ComparisonUnavailable> {
        let panes: [TabId; 2] = self.state.pane_tree.visible_panes().try_into()
            .map_err(|_| ComparisonUnavailable::NeedsTwoFolders)?;
        if panes.iter().any(|&id| self.tab_data_for(id).is_some_and(|tab| tab.is_branch_view)) {
            return Err(ComparisonUnavailable::BranchView);
        }
        panes.iter().all(|&id| {
            !self.tab_closing.contains_key(&id) && !self.pane_closing.contains_key(&id)
                && self.tab_data_for(id).is_some_and(|tab| tab.location.is_directory())
        }).then_some(panes).ok_or(ComparisonUnavailable::NeedsTwoFolders)
    }

    fn comparison_input(&self) -> Option<[DirectoryVersion; 2]> {
        let [left, right] = self.comparison_panes().ok()?;
        let stamp = |pane| {
            let state = self.state.tab_states.get(&pane)?;
            if state.load_state != LoadState::Loaded {
                return None;
            }
            Some(DirectoryVersion {
                pane,
                path: self.tab_data_for(pane)?.location.fs_dir()?.to_path_buf(),
                navigation: state.last_nav_op_id,
                entries: state.entries_version,
            })
        };
        Some([stamp(left)?, stamp(right)?])
    }

    fn invalidate_comparison(&mut self) -> bool {
        self.comparison.generation = self.comparison.generation.wrapping_add(1);
        if let Some((_, cancel)) = &self.comparison.running {
            cancel.store(true, Ordering::Relaxed);
        }
        let had_marks = self.comparison.marks.take().is_some();
        if had_marks {
            if let Some(input) = &self.comparison.input {
                self.dirty_panes.extend(input.iter().map(|dir| dir.pane));
            }
        }
        had_marks
    }

    pub(crate) fn sync_folder_comparison(&mut self) -> bool {
        let input = if self.comparison.enabled {
            self.comparison_input()
        } else {
            None
        };
        let cleared = if input != self.comparison.input {
            let cleared = self.invalidate_comparison();
            self.comparison.input = input;
            cleared
        } else {
            false
        };
        if self.comparison.running.is_some() || self.comparison.marks.is_some() {
            return cleared;
        }
        let Some([left, right]) = &self.comparison.input else {
            return cleared;
        };
        let left = self.state.tab_states[&left.pane].entries.clone();
        let right = self.state.tab_states[&right.pane].entries.clone();
        let generation = self.comparison.generation;
        let verify = self.comparison.verify;
        let cancel = Arc::new(AtomicBool::new(false));
        self.comparison.running = Some((generation, cancel.clone()));
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = atlas_core::fs::folder_compare::compare(&left, &right, verify, &cancel);
            let _ = tx.send(BgResult::FolderCompared { generation, result });
        });
        true
    }

    pub(crate) fn accept_folder_comparison(&mut self, generation: u64, result: Comparison) {
        if self
            .comparison
            .running
            .as_ref()
            .is_none_or(|(current, _)| *current != generation)
        {
            return;
        }
        self.comparison.running = None;
        if generation == self.comparison.generation && self.comparison_input() == self.comparison.input {
            if let Some(input) = &self.comparison.input {
                self.dirty_panes.extend(input.iter().map(|dir| dir.pane));
            }
            self.comparison.marks = Some(result);
        }
    }

    fn comparison_mark(&self, pane: TabId, path: &Path) -> Option<Mark> {
        let input = self.comparison.input.as_ref()?;
        let target = input.iter().position(|dir| dir.pane == pane)?;
        let state = self.state.tab_states.get(&pane)?;
        if state.load_state != LoadState::Loaded
            || state.entries_version != input[target].entries
            || state.last_nav_op_id != input[target].navigation
        {
            return None;
        }
        let marks = self.comparison.marks.as_ref()?;
        if target == 0 {
            marks.left.get(path).copied()
        } else {
            marks.right.get(path).copied()
        }
    }

    pub(crate) fn sync_comparison_controls(&mut self, cx: &mut Context<Self>) {
        let controls = self.comparison_control_state();
        if self.comparison.painted_controls.replace(controls) != Some(controls) {
            self.notify_all_panes(cx);
        }
    }

    fn comparison_control_state(&self) -> (bool, bool, Result<[TabId; 2], ComparisonUnavailable>, bool, bool) {
        (
            self.comparison.enabled,
            self.comparison.verify,
            self.comparison_panes(),
            self.comparison.input.is_some(),
            self.comparison.running.as_ref().is_some_and(|(generation, _)|
                *generation == self.comparison.generation),
        )
    }

    pub(crate) fn comparison_button(
        &self,
        pane: TabId,
        view: &gpui::Entity<Self>,
        controls: crate::ui::glass::ControlColors,
        windows_theme: bool,
        cx: &gpui::App,
    ) -> AnyElement {
        let (enabled, verify, panes, ready, running) = self.comparison_control_state();
        let available = panes.is_ok();
        let branch_view = panes == Err(ComparisonUnavailable::BranchView);
        let enabled = enabled && !branch_view;
        let theme = cx.theme();
        let accent = theme.accent;
        let color = if enabled {
            accent
        } else if available {
            controls.foreground
        } else {
            controls.muted
        };
        let hover = controls.hover;
        let view = view.clone();
        let menu_view = view.clone();
        let state = if running {
            "Comparing folders…"
        } else if enabled && !available {
            "Comparison paused. Click to disable"
        } else if enabled && !ready {
            "Waiting for both folder listings. Click to disable"
        } else if enabled {
            "Disable folder comparison"
        } else if available {
            "Compare folders"
        } else {
            "Open two folder panes to compare"
        };
        let tip = if branch_view {
            "Folder comparison is unavailable in Branch View. Turn off Branch View in both panes to compare.".to_string()
        } else {
            format!("{state}\nSame: matching metadata by default. With SHA-256 enabled, Same on a file means matching content.\nDifferent: a mismatch. Only here: present in just this pane.\nUnverified entries have no badge. Right-click to toggle SHA-256.")
        };
        let button = div()
            .id(("folder-compare", pane.0))
            .size(crate::ui::rem_px(26.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(crate::ui::rem_px(4.))
            .when(enabled, |button| button.bg(accent.opacity(0.12)))
            .when(enabled || available, |button| {
                button
                    .cursor_pointer()
                    .motion_feedback()
                    .hover(move |button| button.bg(hover))
                    .active(move |button| button.bg(controls.pressed))
            })
            .child(theme_icon(AtlasIcon::FolderCompare, 14., color, windows_theme))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(move |_, _, cx| {
                view.update(cx, |app, cx| {
                    let panes = app.comparison_panes();
                    if panes == Err(ComparisonUnavailable::BranchView)
                        || (!app.comparison.enabled && panes.is_err())
                    {
                        return;
                    }
                    app.comparison.enabled = !app.comparison.enabled;
                    app.sync_folder_comparison();
                    app.flush_dirty_panes(cx);
                    cx.notify();
                });
            });
        if branch_view {
            return button.into_any_element();
        }
        button
            .context_menu(move |menu, _, _| {
                let app = menu_view.clone();
                menu.item(
                    PopupMenuItem::new(if verify {
                        "Turn off SHA-256 verification"
                    } else {
                        "Verify file contents with SHA-256"
                    })
                    .on_click(move |_, _, cx| {
                        app.update(cx, |app, cx| {
                            if app.comparison_panes() == Err(ComparisonUnavailable::BranchView) {
                                return;
                            }
                            app.comparison.verify = !app.comparison.verify;
                            app.invalidate_comparison();
                            app.sync_folder_comparison();
                            app.flush_dirty_panes(cx);
                            cx.notify();
                        })
                    }),
                )
            })
            .into_any_element()
    }
}

pub(crate) fn has_badges(app: &WeakEntity<App>, pane: TabId, cx: &gpui::App) -> bool {
    app.upgrade().is_some_and(|entity| {
        let comparison = &entity.read(cx).comparison;
        comparison.marks.is_some() && comparison.input.as_ref().is_some_and(|input| {
            input.iter().any(|directory| directory.pane == pane)
        })
    })
}

pub(crate) fn paint_badge(
    app: &WeakEntity<App>,
    pane: TabId,
    path: &Path,
    bounds: Bounds<Pixels>,
    zoom: f32,
    window: &mut Window,
    cx: &mut gpui::App,
) -> Pixels {
    let Some(mark) = app.upgrade().and_then(|app| app.read(cx).comparison_mark(pane, path)) else {
        return px(0.);
    };
    let theme = cx.theme();
    let dark = theme.is_dark();
    let (label, tint) = match mark {
        Mark::MetadataMatch | Mark::IdenticalContent => ("Same", if dark { gpui::hsla(0.3998, 1.0, 0.6118, 1.0) } else { gpui::hsla(0.4050, 0.8769, 0.2549, 1.0) }),
        Mark::Different => ("Different", if dark { gpui::hsla(0.1150, 1.0, 0.5, 1.0) } else { gpui::hsla(0.0943, 1.0, 0.2980, 1.0) }),
        Mark::LeftOnly | Mark::RightOnly => ("Only here", if dark { gpui::hsla(0.5314, 1.0, 0.5, 1.0) } else { gpui::hsla(0.5402, 1.0, 0.2843, 1.0) }),
        Mark::Unverified => return px(0.),
    };
    let height = px(17. * zoom);
    let padding = px(4. * zoom);
    let gap = px(4. * zoom);
    let line = shape_text(label, &window.text_style().font(), px(11. * zoom), tint, window);
    let width = px(line_width(&line)) + padding * 2.;
    let reserved = width + gap * 2.;
    if bounds.size.width < reserved + px(24. * zoom) || bounds.size.height < height {
        return px(0.);
    }
    let origin = point(
        bounds.origin.x + bounds.size.width - width - gap,
        bounds.origin.y + (bounds.size.height - height) / 2.,
    );
    let fill = tint.opacity(if dark { 0.13 } else { 0.10 });
    window.paint_quad(gpui::quad(
        Bounds { origin, size: size(width, height) },
        gpui::Corners::all(px(4. * zoom)),
        fill,
        gpui::Edges::all(px(0.)),
        tint,
        Default::default(),
    ));
    let _ = line.paint(point(origin.x + padding, origin.y), height, TextAlign::Left, None, window, cx);
    reserved
}
