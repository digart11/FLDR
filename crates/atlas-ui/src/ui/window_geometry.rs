use atlas_model::settings::WindowGeometry;
use gpui::{point, px, size, App, Bounds, DisplayId, Window, WindowBounds};

pub(crate) fn restore(saved: Option<&WindowGeometry>, cx: &App) -> (WindowBounds, Option<DisplayId>) {
    let fallback = || {
        if let Some(display) = cx.primary_display() {
            let area = display.visible_bounds();
            let width = area.size.width.min(px(1280.));
            let height = area.size.height.min(px(800.));
            let rect = Bounds::new(point(area.origin.x + (area.size.width - width) / 2.,
                area.origin.y + (area.size.height - height) / 2.), size(width, height));
            (WindowBounds::Windowed(rect), Some(display.id()))
        } else { (WindowBounds::centered(size(px(1280.), px(800.)), cx), None) }
    };
    let Some(saved) = saved else { return fallback(); };
    let displays = cx.displays();
    let display = displays.iter().find(|display| {
        display.uuid().ok().map(|uuid| uuid.to_string()).as_ref() == saved.monitor.as_ref()
            && saved.monitor.is_some()
    }).cloned().or_else(|| displays.iter().find(|display| {
        display.bounds().contains(&point(px(saved.x + saved.width / 2.), px(saved.y + 20.)))
    }).cloned()).or_else(|| cx.primary_display());
    let Some(display) = display else { return fallback(); };
    let bounds = display.visible_bounds();
    let Some([x, y, width, height]) = super::window_placement::fit_to_monitor(
        [saved.x, saved.y, saved.width, saved.height],
        [bounds.origin.x.into(), bounds.origin.y.into(), bounds.size.width.into(), bounds.size.height.into()],
    ) else { return fallback(); };
    let rect = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
    (if saved.maximized { WindowBounds::Maximized(rect) } else { WindowBounds::Windowed(rect) }, Some(display.id()))
}

pub(crate) fn capture(window: &Window, cx: &App) -> Option<WindowGeometry> {
    #[cfg(windows)] {
        use raw_window_handle::HasWindowHandle;
        if let Ok(handle) = HasWindowHandle::window_handle(window) {
            if let raw_window_handle::RawWindowHandle::Win32(win) = handle.as_raw() {
                if crate::dnd::windows::is_window_minimized(win.hwnd.get()) { return None; }
            }
        }
    }
    let placement = window.window_bounds();
    let rect = placement.get_bounds();
    let values = [f32::from(rect.origin.x), f32::from(rect.origin.y), f32::from(rect.size.width), f32::from(rect.size.height)];
    // Ignore minimized/sentinel placements; keep the last usable restore bounds.
    if values.iter().any(|v| !v.is_finite()) || values[2] < 100. || values[3] < 100.
        || values[0] < -30000. || values[1] < -30000. { return None; }
    let display = window.display(cx);
    let values = if let Some(display) = &display {
        let area = display.visible_bounds();
        super::window_placement::fit_to_monitor(values,
            [area.origin.x.into(), area.origin.y.into(), area.size.width.into(), area.size.height.into()])?
    } else { values };
    Some(WindowGeometry { x: values[0], y: values[1], width: values[2], height: values[3],
        maximized: matches!(placement, WindowBounds::Maximized(_)),
        monitor: display.and_then(|d| d.uuid().ok()).map(|uuid| uuid.to_string()),
    })
}
