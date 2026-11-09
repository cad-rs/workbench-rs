//! The Settings dialog: a centered modal with one row per registered
//! [`wb::SettingSpec`], grouped by `spec.group`. Changes apply as they are
//! made, so the dialog only has a way to close. A string row opens the
//! text-edit overlay, which draws above this dialog.

use gpui::{AnyElement, Context, Div, div, prelude::*, px};
use workbench_api as wb;
use wb::Color;
use workbench_core::platform_cmds::{SETTINGS_EDIT_BEGIN, SETTINGS_SET};

use crate::dialog::{self, DialogFocus};
use crate::platform_panels::section;
use crate::{SharedCore, col, lock, theme};

pub(crate) fn overlay(
    core: &SharedCore,
    focus: &DialogFocus,
    cx: &mut Context<crate::RootView>,
) -> AnyElement {
    let (title, close_label, rows) = {
        let guard = lock(core);
        let host = &guard.host;
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
    dialog::list("settings", title, close_label, rows, core, focus, cx)
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
            // The brand accent shows its color next to the hex value.
            let swatch = (spec.key == "ui.accent").then(|| {
                let color = theme::parse_hex_rgb(&s)
                    .map_or(col(Color::GRAY), |rgba| gpui::rgba(rgba).into());
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded(px(3.))
                    .bg(color)
                    .border_1()
                    .border_color(col(Color::GRAY))
            });
            // String settings are hand-edited: clicking the value opens the
            // root-level Input overlay.
            let edit_key = spec.key.clone();
            div()
                .flex()
                .items_center()
                .gap_2()
                .children(swatch)
                .child(div().text_size(px(12.)).text_color(col(Color::LIGHT_GRAY)).child(s))
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Left, move |_ev, _window, _cx| {
                    wb::dispatch(
                        SETTINGS_EDIT_BEGIN,
                        wb::CommandArgs::new().with("key", edit_key.clone()),
                    );
                })
        }
    };

    dialog::row(tr(&spec.label)).child(control)
}

/// Pressing `element` sets the setting `key` to `value`.
fn sets_on_press(element: Div, key: String, value: String) -> Div {
    element.on_mouse_down(gpui::MouseButton::Left, move |_ev, _window, _cx| {
        wb::dispatch(
            SETTINGS_SET,
            wb::CommandArgs::new().with("key", key.clone()).with("value", value.clone()),
        );
    })
}

fn cycle_button(label: String, key: String, value: String) -> Div {
    sets_on_press(dialog::chip().child(label), key, value)
}

/// Stepper `−`/`+` button for int settings.
fn step_button(label: &str, key: String, value: i64) -> Div {
    sets_on_press(
        div()
            .w(px(20.))
            .h(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .bg(col(theme::CONTROL_BG))
            .text_size(px(12.))
            .text_color(col(Color::LIGHT_GRAY))
            .child(label.to_string()),
        key,
        value.to_string(),
    )
}
