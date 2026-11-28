//! Parent is a navigation control, deliberately outside FileEntry/virtual-list indices.
use crate::{
    app::App,
    input::action::Action,
    ui::icons::{is_windows_theme, theme_icon, AtlasIcon},
};
use atlas_model::pane::TabId;
use gpui::{prelude::*, px, AnyElement, ColorExt as _, Context, MouseButton, SharedString, Window};
use gpui_component::{h_flex, ActiveTheme};

impl App {
    pub(crate) fn parent_entry_is_focused(&self, window: &Window) -> bool {
        let pane = self.state.focused_tile;
        self.parent_entry_focus
            .get(&pane)
            .is_some_and(|f| f.is_focused(window))
            && self.tab_data_for(pane).is_some_and(|t| {
                super::grid_presentation::parent_entry_visible(&t.view_mode) && super::navigation_policy::parent_path(&t.location, &t.view_mode).is_some()
            })
    }

    pub(crate) fn render_parent_entry(
        &mut self,
        pane: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tab = self.tab_data_for(pane)?;
        if !super::grid_presentation::parent_entry_visible(&tab.view_mode) { return None; }
        super::navigation_policy::parent_path(&tab.location, &tab.view_mode)?;
        let live: std::collections::HashSet<_> = self.state.tab_states.keys().copied().collect();
        self.parent_entry_focus.retain(|id, _| live.contains(id));
        let focus = self
            .parent_entry_focus
            .entry(pane)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let focused = focus.is_focused(window);
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().accent.opacity(0.12);
        let app = cx.entity();
        let outside = app.clone();
        let escape = app.clone();
        let row = h_flex()
            .id(SharedString::from(format!("parent-entry-{}", pane.0)))
            .track_focus(&focus)
            .w_full()
            .h(crate::ui::rem_px(26.))
            .flex_none()
            .px_3()
            .gap_2()
            .text_color(muted)
            .cursor_pointer()
            .when(focused, |e| e.bg(hover))
            .hover(move |e| e.bg(hover))
            .child(theme_icon(
                AtlasIcon::ArrowUp,
                14.,
                muted,
                is_windows_theme(&self.state.profiles.active().theme),
            ))
            .child("..")
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                app.update(cx, |app, cx| {
                    app.activate_tab(pane);
                    if let Some(ts) = app.state.tab_states.get_mut(&pane) {
                        ts.selection.clear();
                    }
                    if ev.click_count >= 2 {
                        app.process_actions([Action::NavigateUp(pane)]);
                        window.focus(&app.focus_handle, cx);
                    } else {
                        window.focus(&focus, cx);
                    }
                    app.notify_pane(pane, cx);
                    cx.notify();
                });
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(move |_, window, cx| {
                outside.update(cx, |app, cx| {
                    if app.state.focused_tile == pane && app.parent_entry_is_focused(window) {
                        window.focus(&app.focus_handle, cx);
                        app.notify_pane(pane, cx);
                    }
                });
            })
            .capture_action(move |_: &crate::CloseOverlays, window, cx| {
                escape.update(cx, |app, cx| {
                    window.focus(&app.focus_handle, cx);
                    app.notify_pane(pane, cx);
                });
                cx.stop_propagation();
            })
            .min_w(px(0.));
        // Static Atlas arrow bindings dispatch actions before raw key callbacks.
        let view = cx.entity();
        let row = leave_on::<_, crate::NavUp>(row, &view, pane);
        let row = leave_on::<_, crate::NavDown>(row, &view, pane);
        let row = leave_on::<_, crate::NavHome>(row, &view, pane);
        let row = leave_on::<_, crate::NavEnd>(row, &view, pane);
        let row = leave_on::<_, crate::NavPageUp>(row, &view, pane);
        let row = leave_on::<_, crate::NavPageDown>(row, &view, pane);
        let row = leave_on::<_, crate::ExtendUp>(row, &view, pane);
        let row = leave_on::<_, crate::ExtendDown>(row, &view, pane);
        Some(row.into_any_element())
    }
}

fn leave_on<E: gpui::InteractiveElement, A: gpui::Action>(
    element: E,
    view: &gpui::Entity<App>,
    pane: TabId,
) -> E {
    let view = view.clone();
    element.capture_action(move |_: &A, window, cx| {
        view.update(cx, |app, cx| {
            window.focus(&app.focus_handle, cx);
            app.notify_pane(pane, cx);
        });
        cx.propagate();
    })
}
