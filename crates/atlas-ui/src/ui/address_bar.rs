//! Inline editor backed by the same native text input as the existing search UI.
use crate::{app::App, input::action::Action};
use atlas_model::{location::Location, pane::TabId};
use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, FocusHandle, Focusable, Hsla, KeyDownEvent,
    SharedString, Window,
};
use gpui_component::{h_flex, input::InputState, ActiveTheme};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

// A disconnected share may hold an OS call indefinitely. Never accumulate more
// blocking enumerations behind it; a newer request waits asynchronously.
static ENUMERATING: AtomicBool = AtomicBool::new(false);
struct EnumerationGuard;
impl Drop for EnumerationGuard {
    fn drop(&mut self) {
        ENUMERATING.store(false, Ordering::Release);
    }
}

pub(crate) struct AddressEditor {
    pane: TabId,
    origin: Location,
    input: Entity<InputState>,
    suggestions: Vec<String>,
    selected: Option<usize>,
    generation: Arc<AtomicU64>,
    task: Option<gpui::Task<()>>,
    _subscription: gpui::Subscription,
    dropdown_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
}

impl Drop for AddressEditor {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl App {
    pub(crate) fn address_focus_handle(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<FocusHandle> {
        let pane = self.state.focused_tile;
        let origin = self.tab_data_for(pane)?.location.clone();
        let state = &self.state.tab_states.get(&pane)?.path_edit;
        let text = state.text.clone();
        let select = state.focus;
        if !self
            .address_editor
            .as_ref()
            .is_some_and(|e| e.pane == pane && e.origin == origin)
        {
            let input = crate::ui::text_input::single_line(window, cx, "Path", &text);
            let subscription = cx.subscribe_in(
                &input,
                window,
                |app, _, event: &gpui_component::input::InputEvent, window, cx| {
                    if matches!(event, gpui_component::input::InputEvent::Change) {
                        app.request_address_completions(window, cx);
                    }
                },
            );
            self.address_editor = Some(AddressEditor {
                pane,
                origin,
                input,
                suggestions: Vec::new(),
                selected: None,
                generation: Arc::new(AtomicU64::new(0)),
                task: None,
                _subscription: subscription,
                dropdown_bounds: Default::default(),
            });
            self.notify_pane(pane, cx);
        }
        let input = self.address_editor.as_ref()?.input.clone();
        if select {
            input.update(cx, |input, cx| {
                input.set_value(text.clone(), window, cx);
                input.select_leading(text.chars().count() as u32, cx);
                input.set_inline_suffix(None, cx);
            });
            if let Some(editor) = self.address_editor.as_mut() {
                editor.generation.fetch_add(1, Ordering::Relaxed);
                editor.task = None;
                editor.suggestions.clear();
                editor.selected = None;
            }
            self.state.tab_states.get_mut(&pane)?.path_edit.focus = false;
        }
        Some(input.read(cx).focus_handle(cx))
    }

    fn request_address_completions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.address_editor.as_mut() else {
            return;
        };
        editor.task = None;
        editor.suggestions.clear();
        editor.selected = None;
        let generation = editor.generation.clone();
        let expected = generation.fetch_add(1, Ordering::Relaxed) + 1;
        let text = editor.input.read(cx).value().to_string();
        editor
            .input
            .update(cx, |input, cx| input.set_inline_suffix(None, cx));
        let query = editor
            .origin
            .fs_dir()
            .and_then(|base| super::path_completion::query(&text, base));
        let pane = editor.pane;
        self.notify_pane(pane, cx);
        let Some(query) = query else {
            return;
        };
        let task = cx.spawn_in(window, async move |app, cx| {
            smol::Timer::after(std::time::Duration::from_millis(180)).await;
            loop {
                if generation.load(Ordering::Relaxed) != expected {
                    return;
                }
                if ENUMERATING
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
                smol::Timer::after(std::time::Duration::from_millis(100)).await;
            }
            let response_generation = generation.clone();
            let guard = EnumerationGuard;
            let worker = cx.background_executor().spawn(async move {
                let _guard = guard;
                super::path_completion::enumerate(query, &generation, expected)
            });
            let suggestions = worker.await;
            let _ = app.update_in(cx, |app, _, cx| {
                let Some(editor) = app.address_editor.as_mut() else {
                    return;
                };
                if editor.pane != pane
                    || !Arc::ptr_eq(&editor.generation, &response_generation)
                    || editor.generation.load(Ordering::Relaxed) != expected
                    || editor.input.read(cx).value().as_ref() != text
                {
                    return;
                }
                let suffix = suggestions
                    .first()
                    .and_then(|s| super::path_completion::suffix(&text, s))
                    .map(str::to_owned);
                editor
                    .input
                    .update(cx, |input, cx| input.set_inline_suffix(suffix, cx));
                editor.suggestions = suggestions;
                editor.task = None;
                app.notify_pane(pane, cx);
                cx.notify();
            });
        });
        if let Some(editor) = self.address_editor.as_mut() {
            editor.task = Some(task);
        }
    }

    fn address_completion_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.address_editor.as_mut() else {
            return;
        };
        let count = editor.suggestions.len();
        editor.selected = editor.selected.filter(|index| *index < count);
        match key {
            "up" | "down" if count > 0 => {
                editor.selected = Some(match (editor.selected, key) {
                    (None, "up") => count - 1,
                    (None, _) => 0,
                    (Some(i), "up") => i.saturating_sub(1),
                    (Some(i), _) => (i + 1).min(count - 1),
                });
                let text = editor.input.read(cx).value();
                let suffix = super::path_completion::suffix(
                    &text,
                    &editor.suggestions[editor.selected.unwrap()],
                )
                .map(str::to_owned);
                editor
                    .input
                    .update(cx, |input, cx| input.set_inline_suffix(suffix, cx));
            }
            "tab" | "enter" if count > 0 && (key == "tab" || editor.selected.is_some()) => {
                let candidate = editor.suggestions[editor.selected.unwrap_or(0)].clone();
                let input = editor.input.read(cx);
                let text = input.value();
                if key == "tab" {
                    if !input.selected_range().is_empty() || input.cursor() != text.len() {
                        return;
                    }
                    let Some(suffix) =
                        super::path_completion::suffix(&text, &candidate).map(str::to_owned)
                    else {
                        return;
                    };
                    editor
                        .input
                        .update(cx, |input, cx| input.insert(&suffix, window, cx));
                } else {
                    let chars = text.chars().count() as u32;
                    editor.input.update(cx, |input, cx| {
                        input.select_leading(chars, cx);
                        input.replace(candidate, window, cx);
                    });
                }
                self.request_address_completions(window, cx);
            }
            "escape" if count > 0 || editor.task.is_some() => {
                editor.generation.fetch_add(1, Ordering::Relaxed);
                editor.task = None;
                editor.suggestions.clear();
                editor.selected = None;
                editor
                    .input
                    .update(cx, |input, cx| input.set_inline_suffix(None, cx));
            }
            "enter" => {
                self.finish_address_edit(true, window, cx);
                return;
            }
            "escape" => {
                self.finish_address_edit(false, window, cx);
                return;
            }
            _ => {}
        }
        self.notify_pane(self.state.focused_tile, cx);
        cx.notify();
    }

    fn finish_address_edit(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.address_editor.as_ref() else {
            return;
        };
        let pane = editor.pane;
        if self.state.focused_tile != pane
            || !self
                .tab_data_for(pane)
                .is_some_and(|t| t.location == editor.origin)
        {
            return;
        }
        let path = if commit {
            let base = editor
                .origin
                .fs_dir()
                .unwrap_or_else(|| std::path::Path::new(""));
            match super::navigation_policy::edited_path(&editor.input.read(cx).value(), base) {
                Ok(path) => Some(path),
                Err(message) => {
                    if let Some(ts) = self.state.tab_states.get_mut(&pane) {
                        ts.path_edit.error = Some(message.into());
                    }
                    self.notify_pane(pane, cx);
                    return;
                }
            }
        } else {
            None
        };
        if let Some(ts) = self.state.tab_states.get_mut(&pane) {
            ts.path_edit.editing = false;
            ts.path_edit.focus = false;
            ts.path_edit.error = None;
        }
        self.address_editor = None;
        if let Some(path) = path {
            self.process_actions([Action::Navigate { tab: pane, path }]);
        }
        window.focus(&self.focus_handle, cx);
        self.notify_pane(pane, cx);
        cx.notify();
    }

    pub(crate) fn render_address_editor(
        &self,
        pane: TabId,
        view: &Entity<App>,
        background: Hsla,
        cx: &gpui::App,
    ) -> Option<AnyElement> {
        let state = &self.state.tab_states.get(&pane)?.path_edit;
        if !state.editing {
            return None;
        }
        let editor = self.address_editor.as_ref().filter(|e| {
            e.pane == pane
                && self
                    .tab_data_for(pane)
                    .is_some_and(|t| t.location == e.origin)
        })?;
        let error = state.error.clone();
        let app = view.clone();
        let clicked = view.clone();
        let enter = view.clone();
        let escape = view.clone();
        let outside = view.clone();
        let up = view.clone();
        let down = view.clone();
        let tab = view.clone();
        let outside_bounds = editor.dropdown_bounds.clone();
        let painted_bounds = editor.dropdown_bounds.clone();
        editor.dropdown_bounds.set(None);
        let dropdown = if editor.suggestions.is_empty()
            || !matches!(
                self.active_claim(),
                Some(super::overlay_focus::OverlayClaim::AddressEdit)
            ) {
            None
        } else {
            Some(
                gpui::deferred(
                    div()
                        .absolute()
                        .top(gpui::relative(1.))
                        .left_0()
                        .w_full()
                        .bg(cx.theme().popover)
                        .text_color(cx.theme().popover_foreground)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .shadow_md()
                        .py_1()
                        .children(editor.suggestions.iter().enumerate().map(|(index, text)| {
                            let view = view.clone();
                            let candidate = text.clone();
                            div()
                                .id(SharedString::from(format!("path-suggestion-{index}")))
                                .px_3()
                                .h(crate::ui::rem_px(26.))
                                .text_ellipsis()
                                .overflow_hidden()
                                .when(editor.selected == Some(index), |row| {
                                    row.bg(cx.theme().list_active)
                                })
                                .hover(|row| row.bg(cx.theme().list_hover))
                                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                    view.update(cx, |app, cx| {
                                        let Some(editor) = app.address_editor.as_mut() else {
                                            return;
                                        };
                                        let Some(index) =
                                            editor.suggestions.iter().position(|s| *s == candidate)
                                        else {
                                            return;
                                        };
                                        editor.selected = Some(index);
                                        app.address_completion_key("enter", window, cx);
                                    });
                                    cx.stop_propagation();
                                })
                                .child(text.clone())
                        }))
                        .child(
                            gpui::canvas(
                                move |bounds, _, _| painted_bounds.set(Some(bounds)),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        ),
                )
                .priority(2),
            )
        };
        let windows = crate::ui::icons::is_windows_theme(&self.state.profiles.active().theme);
        let field_focus = editor.input.read(cx).focus_handle(cx);
        let ink = cx.theme().foreground;
        let field_canvas = gpui::canvas(|_, _, _| {}, move |bounds, _, window, _| {
            let zoom = f32::from(window.rem_size()) / crate::ui::BASE_PX;
            let height = crate::ui::address_surface::height(window, windows);
            let field = gpui::Bounds {
                origin: gpui::point(bounds.left() + px(12.0 * zoom),
                    bounds.top() + px(crate::ui::address_policy::centered_y(0.0, f32::from(bounds.size.height), height))),
                size: gpui::size((bounds.size.width - px(24.0 * zoom)).max(px(0.)), px(height)),
            };
            let state = if field_focus.is_focused(window) { crate::ui::address_policy::FieldState::Focused }
                else if field.contains(&window.mouse_position()) { crate::ui::address_policy::FieldState::Hovered }
                else { crate::ui::address_policy::FieldState::Idle };
            crate::ui::address_surface::paint(window, bounds, Some(field), (if windows {10.0} else {9.0}) * zoom,
                background, ink, state);
        }).absolute().inset_0();
        Some(
            h_flex()
                .id(SharedString::from(format!("address-editor-{}", pane.0)))
                .w_full()
                .relative()
                .min_w(px(0.))
                .h(crate::ui::rem_px(
                    if crate::ui::icons::is_windows_theme(&self.state.profiles.active().theme) {
                        44.
                    } else {
                        36.
                    },
                ))
                .px_3()
                .gap_2()
                .hover(|style| style.cursor_text())
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                    clicked.update(cx, |app, cx| {
                        app.activate_tab(pane);
                        cx.notify();
                    });
                    cx.stop_propagation();
                })
                .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
                // GPUI resolves input key bindings before raw key listeners.
                .capture_action(move |_: &gpui_component::input::Enter, window, cx| {
                    enter.update(cx, |app, cx| {
                        app.address_completion_key("enter", window, cx)
                    });
                    cx.stop_propagation();
                })
                .capture_action(move |_: &gpui_component::input::Escape, window, cx| {
                    escape.update(cx, |app, cx| {
                        app.address_completion_key("escape", window, cx)
                    });
                    cx.stop_propagation();
                })
                .capture_action(move |_: &gpui_component::input::MoveUp, window, cx| {
                    up.update(cx, |app, cx| app.address_completion_key("up", window, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &gpui_component::input::MoveDown, window, cx| {
                    down.update(cx, |app, cx| app.address_completion_key("down", window, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &gpui_component::input::IndentInline, window, cx| {
                    tab.update(cx, |app, cx| app.address_completion_key("tab", window, cx));
                    cx.stop_propagation();
                })
                .on_mouse_down_out(move |event, window, cx| {
                    if outside_bounds
                        .get()
                        .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        return;
                    }
                    outside.update(cx, |app, cx| app.finish_address_edit(false, window, cx));
                })
                .capture_key_down(move |ev, window, cx| {
                    if matches!(ev.keystroke.key.as_str(), "enter" | "escape") {
                        app.update(cx, |app, cx| {
                            app.address_completion_key(&ev.keystroke.key, window, cx)
                        });
                        cx.stop_propagation();
                    }
                })
                .child(field_canvas)
                .children(dropdown)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(crate::ui::address_surface::EditorInput { state: editor.input.clone(), windows }),
                )
                .children(error.map(|error| {
                    div()
                        .max_w(crate::ui::rem_px(180.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(cx.theme().muted_foreground)
                        .child(error)
                }))
                .into_any_element(),
        )
    }

    pub(crate) fn handle_navigation_chrome_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let editing = matches!(
            self.active_claim(),
            Some(super::overlay_focus::OverlayClaim::AddressEdit)
        );
        let keys = &ev.keystroke;
        if editing
            && super::navigation_policy::is_address_shortcut(
                &keys.key,
                keys.modifiers.control,
                keys.modifiers.alt,
                keys.modifiers.shift,
                keys.modifiers.platform,
            )
        {
            self.process_actions([Action::EditLocation(self.state.focused_tile)]);
            return true;
        }
        if !self.modal_overlay_active() && self.parent_entry_is_focused(window) {
            use super::navigation_policy::{parent_key, ParentKey};
            match parent_key(
                &keys.key,
                keys.modifiers.control,
                keys.modifiers.alt,
                keys.modifiers.platform,
            ) {
                ParentKey::Open => {
                    self.process_actions([Action::NavigateUp(self.state.focused_tile)]);
                    window.focus(&self.focus_handle, cx);
                    return true;
                }
                ParentKey::Leave | ParentKey::LeaveAndForward => {
                    window.focus(&self.focus_handle, cx);
                    return parent_key(
                        &keys.key,
                        keys.modifiers.control,
                        keys.modifiers.alt,
                        keys.modifiers.platform,
                    ) == ParentKey::Leave;
                }
                ParentKey::Other => {
                    // Selection/clipboard/type-to-filter commands resume ordinary
                    // file focus; a subsequent Enter must not open the parent.
                    window.focus(&self.focus_handle, cx);
                    self.notify_pane(self.state.focused_tile, cx);
                }
            }
        }
        false
    }
}
