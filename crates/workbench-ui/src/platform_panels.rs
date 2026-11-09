//! Built-in platform panels: the Diagnostics and Plugins management panels.
//!
//! They live here rather than in `workbench-core` because they are view
//! implementations — core owns the platform runtime, the frontend owns every
//! view. Each panel builds its content as a gpui element tree in
//! [`wb::ViewInstance::element`], reading live state through the host it is
//! handed. Registered by [`register`], called from the frontend's assembly
//! stage.

use gpui::{div, prelude::*, px, AnyElement, Div, InteractiveElement, IntoElement};
use gpui_component::scroll::ScrollableElement as _;
use workbench_api as wb;
use wb::{Color, PanelDef, Registry};

use super::col;

/// A section heading inside a panel or dialog.
pub(crate) fn section(title: &str) -> Div {
    div()
        .mt_2()
        .text_size(px(13.))
        .text_color(col(Color::ACCENT))
        .child(title.to_string())
}

/// One `key  value` line.
fn kv(key: &str, value: String) -> Div {
    div()
        .flex()
        .gap_2()
        .text_size(px(12.))
        .child(
            div()
                .w(px(110.))
                .text_color(col(Color::GRAY))
                .child(key.to_string()),
        )
        .child(div().text_color(col(Color::LIGHT_GRAY)).child(value))
}

// ---------------------------------------------------------------- Diagnostics ----

pub struct DiagnosticsPanel;

impl wb::ViewInstance for DiagnosticsPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("app.panel.diagnostics")
    }
    fn title(&self) -> String {
        "Diagnostics".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let host = host.host();
        let tr = |k: &str| host.tr(k);

        let backend = if host.stats.backend.is_empty() {
            tr("(unlabeled)")
        } else {
            host.stats.backend.clone()
        };
        let docs: Vec<(String, bool, u64)> = host
            .docs
            .docs()
            .map(|d| (d.title.clone(), d.dirty, d.revision))
            .collect();
        let dirty = docs.iter().filter(|(_, d, _)| *d).count();
        let (running, total) = (host.tasks.summary().0, host.tasks.all().count());
        let interval = host.settings.get_f64("autosave.interval_secs", 30.0);

        let rows: Vec<Div> = vec![
            section(&tr("Runtime")),
            kv(&tr("Product"), tr("(see window title)")),
            kv(&tr("GUI backend"), backend),
            kv(&tr("Uptime"), format!("{:.1} {}", host.uptime_secs(), tr("s"))),
            kv(
                &tr("Frame stats"),
                format!(
                    "{:.1} {}{:.1}) × {}",
                    host.stats.avg_frame_ms,
                    tr("ms/frame (recent "),
                    host.stats.last_frame_ms,
                    host.stats.frames
                ),
            ),
            section(&tr("Documents & tasks")),
            kv(&tr("Documents"), format!("{}", docs.len())),
            kv(&tr("Unsaved"), format!("{dirty}")),
            kv(
                &tr("Background tasks"),
                format!("{}{running}{}{total}", tr("running "), tr(" / total ")),
            ),
            section(&tr("Autosave")),
            kv(
                &tr("Interval"),
                format!(
                    "{interval:.0} {}",
                    tr("s (tunable in settings.json)")
                ),
            ),
            kv(
                &tr("Autosaved"),
                format!("{}", host.stats.autosaves),
            ),
        ];

        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .p_3()
            .children(rows)
            .overflow_y_scrollbar()
            .into_any_element();
        Some(Box::new(element))
    }
}

// ---------------------------------------------------------------- Plugins ----

pub struct PluginsPanel;

impl wb::ViewInstance for PluginsPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("app.panel.plugins")
    }
    fn title(&self) -> String {
        "Plugins".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let host = host.host();
        let plugins: Vec<wb::PluginEntry> = host.plugins.clone();
        let tr = |k: &str| host.tr(k);

        let rows: Vec<Div> = if plugins.is_empty() {
            vec![div()
                .text_size(px(13.))
                .text_color(col(Color::GRAY))
                .child(tr("No plugins registered"))]
        } else {
            plugins
                .iter()
                .map(|entry| {
                    let (color, state_key) = match entry.state {
                        "Loaded" => (Color::GREEN, "Loaded"),
                        "Disabled" => (Color::YELLOW, "Disabled"),
                        _ => (Color::RED, "Rejected"),
                    };
                    let mut row = div()
                        .flex()
                        .gap_2()
                        .text_size(px(12.))
                        .child(div().text_color(col(Color::WHITE)).child(entry.id.clone()))
                        .child(
                            div()
                                .w(px(72.))
                                .text_color(col(color))
                                .child(tr(state_key)),
                        );
                    // The feature the plugin requires, when the manifest names one.
                    if let Some(feature) = &entry.requires {
                        row = row.child(
                            div()
                                .px_1p5()
                                .rounded(px(3.))
                                .bg(col(Color::ACCENT))
                                .text_size(px(10.))
                                .text_color(col(Color::WHITE))
                                .child(feature.clone()),
                        );
                    }
                    if let Some(detail) = &entry.detail {
                        row = row.child(
                            div()
                                .text_size(px(11.))
                                .text_color(col(Color::GRAY))
                                .child(format!("· {detail}")),
                        );
                    }
                    if entry.state == "Loaded" {
                        let id = entry.id.clone();
                        let disable_label = tr("Disable");
                        row.child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded(px(3.))
                                .bg(col(crate::theme::CONTROL_BG))
                                .text_size(px(11.))
                                .child(disable_label)
                                .on_mouse_down(gpui::MouseButton::Left, move |_ev, _window, _cx| {
                                    wb::dispatch(
                                        "app.plugins.disable",
                                        wb::CommandArgs::new().with("id", id.clone()),
                                    );
                                }),
                        )
                    } else {
                        row
                    }
                })
                .collect()
        };

        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .children(rows)
            .overflow_y_scrollbar()
            .into_any_element();
        Some(Box::new(element))
    }
}

/// Register the two platform panels into a product's registry.
///
/// Called from the frontend's assembly stage, before `platform_cmds::register`
/// builds the "View" toolbar group — so the group picks these up with the rest.
pub fn register(reg: &mut Registry) {
    reg.register_panel(
        PanelDef::new(
            "app.panel.diagnostics",
            "Diagnostics",
            wb::DockArea::Right,
            320.0,
            2,
            || Box::new(DiagnosticsPanel),
        )
        .icon(workbench_core::assets::icons::DIAGNOSTICS),
    );
    reg.register_panel(
        PanelDef::new(
            "app.panel.plugins",
            "Plugins",
            wb::DockArea::Right,
            320.0,
            3,
            || Box::new(PluginsPanel),
        )
        .icon(workbench_core::assets::icons::PLUGINS),
    );
}
