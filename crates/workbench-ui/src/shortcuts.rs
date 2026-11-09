//! The Commands & Shortcuts dialog: what the user can run and the key for it.
//!
//! Rendered by the root view while `ws.shortcuts_open` is set (the
//! `app.shortcuts.open` platform command sets it).

use gpui::{AnyElement, Context, Div, Window, div, prelude::*, px};
use workbench_api as wb;
use wb::Color;

use crate::platform_panels::section;
use crate::{SharedCore, col, dialog};

pub(crate) fn is_open(core: &SharedCore) -> bool {
    core.lock().unwrap_or_else(|p| p.into_inner()).host.ws.shortcuts_open
}

fn close(core: &SharedCore) {
    core.lock().unwrap_or_else(|p| p.into_inner()).host.ws.shortcuts_open = false;
}

/// The dialog, while it is open.
pub(crate) fn overlay(
    core: &SharedCore,
    focus: &mut dialog::DialogFocus,
    window: &mut Window,
    cx: &mut Context<crate::RootView>,
) -> Option<AnyElement> {
    focus.sync(is_open(core), true, window, cx);

    let (title, close_label, rows) = {
        let guard = core.lock().unwrap_or_else(|p| p.into_inner());
        if !guard.host.ws.shortcuts_open {
            return None;
        }
        let tr = |key: &str| guard.host.tr(key);
        let mut rows: Vec<Div> = Vec::new();
        let mut group = String::new();
        for entry in guard.command_listing() {
            if entry.group != group {
                rows.push(section(&tr(&entry.group)));
                group = entry.group;
            }
            rows.push(row(tr(&entry.label), entry.hotkey));
        }
        (tr("Commands & Shortcuts"), tr("Close"), rows)
    };

    let core = core.clone();
    Some(dialog::list("shortcuts", title, close_label, rows, focus, move || close(&core), cx))
}

/// One `command  key` row; a command without a key shows only its name.
fn row(label: String, hotkey: Option<String>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_1()
        .py_0p5()
        .child(div().text_size(px(12.)).text_color(col(Color::GRAY)).child(label))
        .children(hotkey.map(|key| {
            div()
                .px_2()
                .py_0p5()
                .rounded(px(3.))
                .bg(col(Color::rgb(0.30, 0.31, 0.36)))
                .text_size(px(12.))
                .text_color(col(Color::LIGHT_GRAY))
                .child(key)
        }))
}
