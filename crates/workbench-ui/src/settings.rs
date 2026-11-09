//! The Settings dialog: a centered modal with one row per registered
//! [`wb::SettingSpec`], grouped by `spec.group`.
//!
//! Rendered by the root view while `ws.settings_open` is set (the
//! `app.settings.open` platform command sets it). Changes apply as they are
//! made, so the dialog only has a way to close. A string row opens the
//! text-edit overlay, which draws above this dialog.

use gpui::{AnyElement, Context, Div, Window, div, prelude::*, px};
use workbench_api as wb;
use wb::Color;

use crate::platform_panels::section;
use crate::{SharedCore, col, dialog};

pub(crate) fn is_open(core: &SharedCore) -> bool {
    core.lock().unwrap_or_else(|p| p.into_inner()).host.ws.settings_open
}

fn close(core: &SharedCore) {
    core.lock().unwrap_or_else(|p| p.into_inner()).host.ws.settings_open = false;
}

/// The dialog, while it is open.
pub(crate) fn overlay(
    core: &SharedCore,
    focus: &mut dialog::DialogFocus,
    window: &mut Window,
    cx: &mut Context<crate::RootView>,
) -> Option<AnyElement> {
    let on_top = !crate::text_edit::is_active(core) && !crate::file_picker::is_open(core);
    focus.sync(is_open(core), on_top, window, cx);

    let (title, close_label, rows) = {
        let guard = core.lock().unwrap_or_else(|p| p.into_inner());
        let host = &guard.host;
        if !host.ws.settings_open {
            return None;
        }
        let tr = |key: &str| host.tr(key);
        let mut rows: Vec<Div> = Vec::new();
        let mut group = String::new();
        for spec in &host.setting_specs {
            if spec.group != group {
                group = spec.group.clone();
                rows.push(section(&tr(&group)));
            }
            rows.push(setting_row(spec, &host.settings, &tr));
        }
        (tr("Settings"), tr("Close"), rows)
    };

    let core = core.clone();
    Some(dialog::list("settings", title, close_label, rows, focus, move || close(&core), cx))
}

/// One `label  control` row for a setting spec. `tr` translates display
/// strings (labels and enum/bool display names).
fn setting_row(
    spec: &wb::SettingSpec,
    settings: &wb::Settings,
    tr: &dyn Fn(&str) -> String,
) -> Div {
    let value = settings
        .get(&spec.key)
        .cloned()
        .unwrap_or_else(|| spec.effective_default());

    let control: Div = match &spec.kind {
        wb::SettingKind::Bool => {
            let on = value.as_bool().unwrap_or(false);
            cycle_button(
                tr(if on { "On" } else { "Off" }),
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
            // Known enum values get friendly display names; anything else
            // (plugin-defined options) shows the raw value.
            let display = match current.as_str() {
                "dark" => tr("Dark"),
                "light" => tr("Light"),
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
            // String settings are hand-edited: clicking the value opens the
            // root-level Input overlay (app.settings.edit_begin).
            let edit_key = spec.key.clone();
            let mut value_row = if spec.key == "ui.accent" {
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
            };
            value_row = value_row.cursor_pointer().on_mouse_down(
                gpui::MouseButton::Left,
                move |_ev, _window, _cx| {
                    wb::dispatch(
                        "app.settings.edit_begin",
                        wb::CommandArgs::new().with("key", edit_key.clone()),
                    );
                },
            );
            value_row
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
                .child(tr(&spec.label)),
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
