//! Hover expansion with a short transit grace period; click/touch retain a pinned popover.
use crate::app::App;
use crate::ui::glass::control_colors;
use crate::ui::icons::{is_windows_theme, theme_icon, AtlasIcon};
use crate::ui::pointer_state::HoverLatch;
use atlas_model::{pane::TabId, settings::ViewMode};
use gpui::prelude::*;
use gpui::{px, AnyElement, Context, Entity, SharedString, Window};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::popover::Popover;
use gpui_component::scroll::ScrollableElement;
use gpui_component::slider::{Slider, SliderState, SliderValue};
use gpui::ColorExt as _;
use gpui_component::{h_flex, v_flex, ActiveTheme, Selectable, Sizable};

use super::view_selector_policy::MODES;

fn mode_label_icon(mode: &ViewMode) -> (&'static str, AtlasIcon) {
    match mode {
        ViewMode::Details => ("view_details", AtlasIcon::Details),
        ViewMode::Grid => ("view_grid", AtlasIcon::Grid),
        ViewMode::List => ("view_list", AtlasIcon::List),
        ViewMode::Miller => ("view_columns", AtlasIcon::Columns),
        ViewMode::Tree => ("view_tree", AtlasIcon::TreeStructure),
    }
}

impl App {
    pub(crate) fn selector_hover(&mut self, pane: TabId, inside: bool, cx: &mut Context<Self>) {
        if inside && (self.rubber.is_some() || cx.has_active_drag()) && !self.open_view_selectors.contains(&pane) {
            return;
        }
        if inside {
            let others: Vec<_> = self.open_view_selectors.iter().copied().filter(|id| *id != pane).collect();
            for other in others {
                self.open_view_selectors.remove(&other);
                if let Some(state) = self.view_selector_hover.get_mut(&other) { *state = HoverLatch::default(); }
                self.notify_pane(other, cx);
            }
        }
        let latch = self.view_selector_hover.entry(pane).or_default();
        let token = latch.touch();
        latch.inside = inside;
        if inside {
            if self.open_view_selectors.insert(pane) {
                self.notify_pane(pane, cx);
            }
        } else if !latch.pinned && !latch.interacting {
            cx.spawn(async move |this, cx| {
                smol::Timer::after(std::time::Duration::from_millis(220)).await;
                let _ = this.update(cx, |app, cx| {
                    if app
                        .view_selector_hover
                        .get(&pane)
                        .is_some_and(|state| state.should_close(token))
                        && app.open_view_selectors.remove(&pane)
                    {
                        app.notify_pane(pane, cx);
                    }
                });
            })
            .detach();
        }
    }

    fn selector_click(&mut self, pane: TabId, close: bool, cx: &mut Context<Self>) -> bool {
        // Use the rendered trigger state: Popover's outside handler may have
        // dismissed the overlay before this same trigger press is delivered.
        if close {
            self.open_view_selectors.remove(&pane);
            *self.view_selector_hover.entry(pane).or_default() = HoverLatch::default();
            self.notify_pane(pane, cx);
            false
        } else {
            self.selector_hover(pane, true, cx);
            self.view_selector_hover.entry(pane).or_default().pinned = true;
            self.notify_pane(pane, cx);
            true
        }
    }

    pub(crate) fn render_view_selector(
        &mut self,
        pane: TabId,
        slider: Entity<SliderState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.view_selector_hover
            .retain(|pane, _| self.state.tab_states.contains_key(pane));
        self.open_view_selectors
            .retain(|pane| self.state.tab_states.contains_key(pane));
        let mode = self
            .tab_data_for(pane)
            .map(|tab| tab.view_mode.clone())
            .unwrap_or_default();
        let (label, glyph) = mode_label_icon(&mode);
        let windows = is_windows_theme(&self.state.profiles.active().theme);
        let colors = control_colors(
            cx.theme(),
            self.state.profiles.active().theme.ends_with(" Glass"),
        );
        let view = cx.entity();
        let hover = view.clone();
        let changes = view.clone();
        let zoom = f32::from(window.rem_size()) / crate::ui::BASE_PX;
        let open = self.open_view_selectors.contains(&pane);
        if open { gpui_component::tooltip::Tooltip::dismiss(window, cx); }
        let click_closes = open && self.view_selector_hover.get(&pane).is_some_and(|latch| latch.pinned);
        let focus = window.use_keyed_state(SharedString::from(format!("view-mode-focus-{pane:?}")), cx, |_, cx| cx.focus_handle()).read(cx).clone();
        let press = view.clone();
        let press_focus = focus.clone();
        let key_view = view.clone();
        let key_focus = focus.clone();
        let trigger = Button::new(SharedString::from(format!("view-select-{pane:?}")))
            .ghost().xsmall()
            .child(theme_icon(glyph, 14., colors.foreground, windows))
            .when(!open, |button| button.tooltip(format!("{} \u{2014} {}", rust_i18n::t!(label), rust_i18n::t!("view_mode_hover_hint"))))
            .on_hover(move |inside, window, cx| {
                if *inside {
                    // Managed tooltip hover callbacks can run after this callback
                    // and arm a timer from the old, closed trigger render.
                    window.defer(cx, |window, cx| gpui_component::tooltip::Tooltip::dismiss(window, cx));
                }
                hover.update(cx, |app, cx| app.selector_hover(pane, *inside, cx))
            })
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                let opened = press.update(cx, |app, cx| app.selector_click(pane, click_closes, cx));
                if opened { press_focus.focus(window, cx); }
                else { press.read(cx).focus_handle.clone().focus(window, cx); }
            })
            .on_click(move |event, window, cx| {
                if matches!(event, gpui::ClickEvent::Mouse(_)) { return; }
                let opened = key_view.update(cx, |app, cx| app.selector_click(pane, click_closes, cx));
                if opened { key_focus.focus(window, cx); }
                else { key_view.read(cx).focus_handle.clone().focus(window, cx); }
            });
        Popover::new(SharedString::from(format!("view-mode-popover-{pane:?}")))
            .anchor(gpui::Anchor::BottomRight)
            .open(open)
            .track_focus(&focus)
            .overlay_closable(!self.view_selector_hover.get(&pane).is_some_and(|state| state.interacting))
            .trigger(trigger)
            .on_open_change(move |open, window, cx| {
                changes.update(cx, |app, cx| {
                    let state: &mut HoverLatch = app.view_selector_hover.entry(pane).or_default();
                    if !*open && state.interacting {
                        // A child slider can move focus while retaining pointer capture.
                        // Reopen the controlled popover until its release event arrives.
                        app.open_view_selectors.insert(pane);
                        app.notify_pane(pane, cx);
                        return;
                    }
                    state.pinned = *open;
                    state.touch();
                    if *open {
                        gpui_component::tooltip::Tooltip::dismiss(window, cx);
                        app.open_view_selectors.insert(pane);
                    } else {
                        app.open_view_selectors.remove(&pane);
                        state.inside = false;
                        app.focus_handle.clone().focus(window, cx);
                    }
                    app.notify_pane(pane, cx);
                });
            })
            .content(move |state, window, cx| {
                let popover = cx.entity();
                let current = view
                    .read(cx)
                    .tab_data_for(pane)
                    .map(|tab| tab.view_mode.clone())
                    .unwrap_or_default();
                let enter = view.clone();
                let slider_press = view.clone();
                let escape_view = view.clone();
                let escape_popover = popover.clone();
                let release_view = view.clone();
                let cancel_view = view.clone();
                let cancel_popover = popover.clone();
                let cancel_action = cx.build_action("ui::Cancel", None).ok();
                let scroll = window
                    .use_keyed_state(
                        SharedString::from(format!("view-mode-scroll-{pane:?}")),
                        cx,
                        |_, _| gpui::ScrollHandle::new(),
                    )
                    .read(cx)
                    .clone();
                let mut rows = v_flex().gap_0p5().w_full().min_w_0();
                for mode in MODES {
                    let (label, glyph) = mode_label_icon(&mode);
                    let app = view.clone();
                    let slider = slider.clone();
                    let close = popover.clone();
                    let active = mode == current;
                    rows = rows.child(
                        Button::new(SharedString::from(format!("view-mode-{pane:?}-{mode:?}")))
                            .ghost()
                            .small()
                            .w_full()
                            .selected(active)
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .items_center()
                                    .child(gpui::div().w(crate::ui::rem_px(3.)).h(crate::ui::rem_px(18.))
                                        .rounded_sm().when(active, |bar| bar.bg(colors.foreground)))
                                    .child(theme_icon(glyph, 14., colors.foreground, windows))
                                    .child(
                                        gpui::div()
                                            .flex_1()
                                            .text_xs()
                                            .child(rust_i18n::t!(label).into_owned()),
                                    )
                            )
                            .on_click(move |_, window, cx| {
                                let value = app.update(cx, |app, cx| {
                                    app.activate_tab(pane);
                                    app.set_pane_view_mode(pane, mode.clone());
                                    app.notify_pane(pane, cx);
                                    app.pane_view_slider_value(pane)
                                });
                                slider.update(cx, |slider, cx| {
                                    slider.set_value(SliderValue::Single(value.max(3.)), window, cx)
                                });
                                close.update(cx, |state, cx| state.dismiss(window, cx));
                                app.read(cx).focus_handle.clone().focus(window, cx);
                            }),
                    );
                }
                let _ = state;
                v_flex()
                    .id(SharedString::from(format!("view-mode-content-{pane:?}")))
                    .gap_2()
                    .items_stretch()
                    .w(crate::ui::rem_px(184.))
                    .min_w_0()
                    .max_w(px((f32::from(window.viewport_size().width) - 40.).max(40.)))
                    .track_focus(&focus)
                    .when_some(cancel_action, |content, action| content.on_boxed_action(action.as_ref(), move |_, window, cx| {
                        dismiss_selector(&cancel_view, &cancel_popover, pane, window, cx);
                    }))
                    .child(gpui::canvas(|_, _, _| (), move |_, _, window, _| {
                        let escape_view = escape_view.clone();
                        let escape_popover = escape_popover.clone();
                        window.on_key_event::<gpui::KeyDownEvent>(move |event, phase, window, cx| {
                            if phase != gpui::DispatchPhase::Capture
                                || !escape_view.read(cx).open_view_selectors.contains(&pane)
                                || !matches!(escape_view.read(cx).active_claim(), Some(crate::ui::overlay_focus::OverlayClaim::ViewSelector)) { return; }
                            let key = event.keystroke.key.as_str();
                            if key != "escape" && (event.keystroke.modifiers.control || event.keystroke.modifiers.alt || event.keystroke.modifiers.platform) { return; }
                            if !matches!(key, "escape" | "enter") && escape_view.read(cx).view_selector_hover.get(&pane).is_some_and(|latch| latch.interacting) { return; }
                            if let Some(mode) = escape_view.read(cx).tab_data_for(pane)
                                .and_then(|tab| super::view_selector_policy::keyboard_mode(&tab.view_mode, key)) {
                                escape_view.update(cx, |app, cx| {
                                    app.activate_tab(pane);
                                    app.set_pane_view_mode(pane, mode);
                                    app.view_selector_hover.entry(pane).or_default().pinned = true;
                                    app.notify_pane(pane, cx);
                                });
                                escape_popover.update(cx, |_, cx| cx.notify());
                                cx.stop_propagation();
                                return;
                            }
                            if !matches!(key, "escape" | "enter") { return; }
                            dismiss_selector(&escape_view, &escape_popover, pane, window, cx);
                        });
                        let release_view = release_view.clone();
                        window.on_mouse_event::<gpui::MouseUpEvent>(move |event, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Capture || event.button != gpui::MouseButton::Left { return; }
                            release_view.update(cx, |app, cx| {
                                if let Some(latch) = app.view_selector_hover.get_mut(&pane) {
                                    if !latch.interacting { return; }
                                    latch.interacting = false;
                                    latch.touch();
                                    if !latch.inside { app.selector_hover(pane, false, cx); }
                                }
                            });
                        });
                    }).absolute().size_0())
                    .max_h(px((f32::from(window.viewport_size().height)
                        - 24. * zoom
                        - 16.)
                        .max(1.)))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .child(rows)
                    .when(current == ViewMode::Grid, |content| content.child(
                        v_flex()
                            .id(SharedString::from(format!("view-mode-slider-wrap-{pane:?}")))
                            .w_full().gap_1().pt_2().border_t_1().border_color(colors.foreground.opacity(0.12))
                            .child(gpui::div().text_xs().child(rust_i18n::t!("view_icon_size").into_owned()))
                            .child(Slider::new(&slider).w_full())
                            .capture_any_mouse_down(move |event, _, cx| {
                                if event.button != gpui::MouseButton::Left { return; }
                                slider_press.update(cx, |app, _| {
                                    let latch = app.view_selector_hover.entry(pane).or_default();
                                    latch.interacting = true;
                                    latch.touch();
                                    app.open_view_selectors.insert(pane);
                                });
                            }),
                    ))
                    .vertical_scrollbar(&scroll)
                    .on_hover(move |inside, window, cx| {
                        if *inside { gpui_component::tooltip::Tooltip::dismiss(window, cx); }
                        enter.update(cx, |app, cx| app.selector_hover(pane, *inside, cx))
                    })
            })
            .into_any_element()
    }
}

// Handles both raw fallback keys and Popover's bound Escape action, which GPUI
// dispatches before raw key listeners. The child handles the component action
// during bubbling before Popover itself can dismiss/reopen an active drag.
fn dismiss_selector(view: &Entity<App>, popover: &Entity<gpui_component::popover::PopoverState>, pane: TabId, window: &mut Window, cx: &mut gpui::App) {
    let dragging = view.update(cx, |app, cx| {
        app.open_view_selectors.remove(&pane);
        let latch = app.view_selector_hover.entry(pane).or_default();
        let dragging = latch.interacting;
        *latch = HoverLatch::default();
        app.notify_pane(pane, cx);
        dragging
    });
    if dragging { cx.stop_drag(window); }
    popover.update(cx, |state, cx| state.dismiss(window, cx));
    view.read(cx).focus_handle.clone().focus(window, cx);
    cx.stop_propagation();
}
