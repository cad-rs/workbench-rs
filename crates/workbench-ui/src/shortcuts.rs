//! The Commands & Shortcuts dialog: what the user can run and the key for it.

use gpui::{AnyElement, Context, prelude::*};

use crate::dialog::{self, DialogFocus};
use crate::platform_panels::section;
use crate::{SharedCore, lock};

pub(crate) fn overlay(
    core: &SharedCore,
    focus: &DialogFocus,
    cx: &mut Context<crate::RootView>,
) -> AnyElement {
    let (title, close_label, rows) = {
        let guard = lock(core);
        let tr = |key: &str| guard.host.tr(key);
        let mut rows = Vec::new();
        let mut group = String::new();
        for entry in guard.command_listing() {
            if entry.group != group {
                rows.push(section(&tr(&entry.group)));
                group = entry.group;
            }
            rows.push(
                dialog::row(tr(&entry.label))
                    .children(entry.hotkey.map(|key| dialog::chip().child(key))),
            );
        }
        (tr("Commands & Shortcuts"), tr("Close"), rows)
    };
    dialog::list("shortcuts", title, close_label, rows, core, focus, cx)
}
