//! Built-in platform panels: the Diagnostics and Plugins management panels.
//!
//! They live here rather than in `workbench-core` because they are view
//! implementations — core owns the platform runtime, the frontend owns every
//! view. Each panel builds its content as a gpui element tree in
//! [`wb::ViewInstance::element`], reading live state through the host it is
//! handed. Registered by [`register`], called from the frontend's assembly
//! stage.

use gpui::{div, prelude::*, px, AnyElement, Div, InteractiveElement, IntoElement};
use workbench_api as wb;
use wb::{Color, PanelDef, Registry};

use super::col;

/// A section heading inside a panel.
fn section(title: &str) -> Div {
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

// ---------------------------------------------------------------- 诊断 ----

pub struct DiagnosticsPanel;

impl wb::ViewInstance for DiagnosticsPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("app.panel.diagnostics")
    }
    fn title(&self) -> String {
        "诊断".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let host = host.host();

        let backend = if host.stats.backend.is_empty() {
            "（未标注）".to_string()
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
            section("运行时"),
            kv("产品", "（见窗口标题）".to_string()),
            kv("GUI 后端", backend),
            kv("运行时长", format!("{:.1} 秒", host.uptime_secs())),
            kv(
                "帧统计",
                format!(
                    "{:.1} ms/帧（最近 {:.1}）× {}",
                    host.stats.avg_frame_ms, host.stats.last_frame_ms, host.stats.frames
                ),
            ),
            section("文档与任务"),
            kv("文档数", format!("{}", docs.len())),
            kv("未保存", format!("{dirty} 个")),
            kv("后台任务", format!("运行 {running} / 累计 {total}")),
            section("自动保存"),
            kv(
                "间隔",
                format!("{interval:.0} 秒（settings.json 可调）"),
            ),
            kv("已自动保存", format!("{} 次", host.stats.autosaves)),
        ];

        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .p_3()
            .children(rows)
            .into_any_element();
        Some(Box::new(element))
    }
}

// ---------------------------------------------------------------- 插件 ----

pub struct PluginsPanel;

impl wb::ViewInstance for PluginsPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("app.panel.plugins")
    }
    fn title(&self) -> String {
        "插件".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let plugins: Vec<wb::PluginEntry> = host.host().plugins.clone();

        let rows: Vec<Div> = if plugins.is_empty() {
            vec![div()
                .text_size(px(13.))
                .text_color(col(Color::GRAY))
                .child("没有已注册的插件".to_string())]
        } else {
            plugins
                .iter()
                .map(|entry| {
                    let (color, state_text) = match entry.state {
                        "Loaded" => (Color::GREEN, "已装载"),
                        "Disabled" => (Color::YELLOW, "已禁用"),
                        _ => (Color::RED, "已拒绝"),
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
                                .child(state_text.to_string()),
                        );
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
                        row.child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded(px(3.))
                                .bg(col(Color::rgb(0.30, 0.31, 0.36)))
                                .text_size(px(11.))
                                .child("禁用".to_string())
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
            .into_any_element();
        Some(Box::new(element))
    }
}

// ---------------------------------------------------------------- 设置 ----

/// The settings panel: one row per registered [`wb::SettingSpec`], grouped by
/// `spec.group`. Controls are self-drawn (the panel view has no gpui entity
/// context for component inputs): bool → toggle, enum → cycle, int → stepper,
/// str → read-only value (editing lands with the text-input wiring).
pub struct SettingsPanel;

impl wb::ViewInstance for SettingsPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("app.panel.settings")
    }
    fn title(&self) -> String {
        "设置".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let host = host.host();
        let specs: Vec<wb::SettingSpec> = host.setting_specs.clone();

        let mut rows: Vec<Div> = Vec::new();
        let mut current_group = String::new();
        for spec in &specs {
            if spec.group != current_group {
                current_group = spec.group.clone();
                rows.push(section(&current_group));
            }
            rows.push(setting_row(spec, &host.settings));
        }

        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .p_3()
            .gap_1()
            .children(rows)
            .into_any_element();
        Some(Box::new(element))
    }
}

/// One `label  control` row for a setting spec.
fn setting_row(spec: &wb::SettingSpec, settings: &wb::Settings) -> Div {
    let value = settings
        .get(&spec.key)
        .cloned()
        .unwrap_or_else(|| spec.effective_default());

    let control: Div = match &spec.kind {
        wb::SettingKind::Bool => {
            let on = value.as_bool().unwrap_or(false);
            cycle_button(
                if on { "开" } else { "关" }.to_string(),
                spec.key.clone(),
                if on { "false".to_string() } else { "true".to_string() },
            )
        }
        wb::SettingKind::Enum(options) => {
            let current = value.as_str().unwrap_or_default().to_string();
            let next = options
                .iter()
                .position(|o| *o == current)
                .map(|i| options[(i + 1) % options.len()].clone())
                .unwrap_or_else(|| options[0].clone());
            let display = match current.as_str() {
                "dark" => "深色".to_string(),
                "light" => "浅色".to_string(),
                other => other.to_string(),
            };
            cycle_button(display, spec.key.clone(), next)
        }
        wb::SettingKind::Int { min, max, step } => {
            let n = value.as_i64().unwrap_or(0);
            let mut row = div().flex().items_center().gap_1().child(
                div()
                    .w(px(56.))
                    .text_align(gpui::TextAlign::Center)
                    .text_color(col(Color::LIGHT_GRAY))
                    .child(n.to_string()),
            );
            if n > *min {
                row = row.child(step_button("−", spec.key.clone(), (n - step).max(*min)));
            }
            if n < *max {
                row = row.child(step_button("+", spec.key.clone(), (n + step).min(*max)));
            }
            row
        }
        wb::SettingKind::Str => {
            let s = value.as_str().unwrap_or_default().to_string();
            if spec.key == "ui.accent" {
                // Brand accent: swatch + hex value.
                div().flex().items_center().gap_2().child(
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .rounded(px(3.))
                        .bg(col(parse_hex_color(&s).unwrap_or(Color::GRAY)))
                        .border_1()
                        .border_color(col(Color::GRAY)),
                ).child(
                    div()
                        .text_size(px(12.))
                        .text_color(col(Color::LIGHT_GRAY))
                        .child(s),
                )
            } else {
                div()
                    .text_size(px(12.))
                    .text_color(col(Color::LIGHT_GRAY))
                    .child(s)
            }
        }
    };

    div()
        .flex()
        .items_center()
        .justify_between()
        .px_1()
        .py_0p5()
        .child(
            div()
                .text_size(px(12.))
                .text_color(col(Color::GRAY))
                .child(spec.label.clone()),
        )
        .child(control)
}

/// A small clickable button that dispatches `app.settings.set`.
fn cycle_button(label: String, key: String, value: String) -> Div {
    div()
        .px_2()
        .py_0p5()
        .rounded(px(3.))
        .bg(col(Color::rgb(0.30, 0.31, 0.36)))
        .text_size(px(12.))
        .text_color(col(Color::LIGHT_GRAY))
        .child(label)
        .on_mouse_down(gpui::MouseButton::Left, move |_ev, _window, _cx| {
            wb::dispatch(
                "app.settings.set",
                wb::CommandArgs::new()
                    .with("key", key.clone())
                    .with("value", value.clone()),
            );
        })
}

/// Stepper `−`/`+` button for int settings.
fn step_button(label: &str, key: String, value: i64) -> Div {
    div()
        .w(px(20.))
        .h(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .bg(col(Color::rgb(0.30, 0.31, 0.36)))
        .text_size(px(12.))
        .text_color(col(Color::LIGHT_GRAY))
        .child(label.to_string())
        .on_mouse_down(gpui::MouseButton::Left, move |_ev, _window, _cx| {
            wb::dispatch(
                "app.settings.set",
                wb::CommandArgs::new()
                    .with("key", key.clone())
                    .with("value", value.to_string()),
            );
        })
}

/// Parse `#RRGGBB` (or `RRGGBB`) into a shell color; None on malformed input.
fn parse_hex_color(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
    Some(Color::rgb(r, g, b))
}

/// Register the two platform panels into a product's registry.
///
/// Called from the frontend's assembly stage, before `platform_cmds::register`
/// builds the "窗口" toolbar group — so the group picks these up with the rest.
pub fn register(reg: &mut Registry) {
    reg.register_panel(PanelDef::new(
        "app.panel.diagnostics",
        "诊断",
        wb::DockArea::Right,
        320.0,
        2,
        || Box::new(DiagnosticsPanel),
    ));
    reg.register_panel(PanelDef::new(
        "app.panel.plugins",
        "插件",
        wb::DockArea::Right,
        320.0,
        3,
        || Box::new(PluginsPanel),
    ));
    reg.register_panel(PanelDef::new(
        "app.panel.settings",
        "设置",
        wb::DockArea::Right,
        320.0,
        4,
        || Box::new(SettingsPanel),
    ));
}
