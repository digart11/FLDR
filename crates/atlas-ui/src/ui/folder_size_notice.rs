//! A separate on-demand result surface; the working bottom-left pill is untouched.
use crate::{app::App, ui::{glass::control_colors, icons::{theme_icon, is_windows_theme, AtlasIcon}}};
use atlas_model::pane::TabId;
use gpui::{prelude::*, div, AnyElement, Context, SharedString};
use gpui_component::{h_flex, ActiveTheme, ElementExt};

impl App {
    pub(crate) fn render_folder_size_notice(&self, pane: TabId, cx: &mut Context<Self>) -> Option<AnyElement> {
        let job = self.context_info.scan.as_ref().filter(|job| job.notice_visible(pane))?;
        let glass = self.state.profiles.active().theme.ends_with(" Glass");
        let windows = is_windows_theme(&self.state.profiles.active().theme);
        let colors = control_colors(cx.theme(), glass);
        let mut surface = colors.surface;
        surface.alpha = if glass { 0.90 } else { 1.0 };
        let running = !job.report.done;
        let text = rust_i18n::t!(job.message_key(),
            size = atlas_core::util::display::format_size(job.report.bytes), files = job.report.files,
            errors = job.report.errors, skipped = job.report.skipped).into_owned();
        let scope = job.paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("\n");
        let tooltip = format!("{text}\n{scope}{}", job.report.first_error.as_ref().map_or(String::new(), |error| format!("\n{error}")));
        let close = cx.entity();
        let generation = job.generation;
        let bottom = if self.state.tab_states.get(&pane).is_some_and(|tab| tab.search.active) { 88. } else { 48. };
        Some(h_flex().id(SharedString::from(format!("folder-size-notice-{}", pane.0)))
            .absolute().bottom(crate::ui::rem_px(bottom)).right_2()
            .max_w(gpui::relative(0.95)).min_h(crate::ui::rem_px(30.))
            .px_2().gap_2().items_center().rounded_md().bg(surface)
            .border_1().border_color(colors.border).text_xs().text_color(colors.foreground)
            .whitespace_nowrap().overflow_hidden()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(theme_icon(AtlasIcon::FolderMeasure, 14., colors.foreground, windows))
            .child(div().id(SharedString::from(format!("folder-size-result-{}", pane.0)))
                .min_w_0().truncate().child(text)
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)))
            .child(div().id(SharedString::from(format!("folder-size-dismiss-{}", pane.0)))
                .flex_none().size(crate::ui::rem_px(24.)).flex().items_center().justify_center().rounded_md()
                .cursor_pointer().hover(move |style| style.bg(colors.hover))
                .child(theme_icon(AtlasIcon::X, 13., colors.foreground, windows))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(
                    if running { rust_i18n::t!("size_cancel_hint").into_owned() } else { rust_i18n::t!("sp_hint_close").into_owned() }).build(window, cx))
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    close.update(cx, |app, cx| {
                        if let Some(job) = app.context_info.scan.as_mut().filter(|job| job.tab == pane && job.generation == generation) {
                            if job.report.done { job.dismiss(); } else { job.request_cancel(); }
                        }
                        app.notify_pane(pane, cx); cx.notify();
                    });
                })).into_any_element())
    }
}
