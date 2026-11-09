//! Asks what to do with the unsaved documents when the window is closing.
//! Closing the dialog any other way (Cancel, Escape, the backdrop) keeps the
//! app open.

use gpui::{AnyElement, ClickEvent, Context, Window, div, prelude::*, px};
use gpui_component::Sizable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use workbench_api as wb;
use workbench_core::platform_cmds;

use crate::dialog::{self, DialogFocus};
use crate::{RootView, SharedCore, lock};

pub(crate) fn overlay(
    core: &SharedCore,
    focus: &DialogFocus,
    cx: &mut Context<RootView>,
) -> AnyElement {
    let (titles, heading, save, dont_save, cancel) = {
        let guard = lock(core);
        let host = &guard.host;
        let titles: Vec<String> =
            host.docs.docs().filter(|doc| doc.dirty).map(|doc| doc.title.clone()).collect();
        (
            titles,
            host.tr("Save your changes before closing?"),
            host.tr("Save"),
            host.tr("Don't Save"),
            host.tr("Cancel"),
        )
    };

    let names = titles.into_iter().map(dialog::row);

    let core_save = core.clone();
    let save = button("unsaved-save", save, cx, move || {
        platform_cmds::save_all_and_close(&mut wb::CommandCtx::new(&mut *lock(&core_save)));
    })
    .primary();
    let core_discard = core.clone();
    let dont_save = button("unsaved-discard", dont_save, cx, move || {
        let mut guard = lock(&core_discard);
        guard.host.ws.dialog = None;
        guard.host.ws.close_approved = true;
    })
    .outline();
    let core_cancel = core.clone();
    let cancel =
        button("unsaved-cancel", cancel, cx, move || dialog::close(&core_cancel)).outline();

    let (backdrop, card) = dialog::platform_shell("unsaved", 420.0, core, focus, cx);
    let card = card
        .p(px(20.0))
        .gap(px(12.0))
        .child(div().text_size(px(16.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(heading))
        .child(div().flex().flex_col().gap_1().max_h(px(200.0)).children(names).overflow_y_scrollbar())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(dont_save)
                .child(div().flex().gap_2().child(cancel).child(save)),
        );
    backdrop.child(card).into_any_element()
}

fn button(
    id: &'static str,
    label: String,
    cx: &mut Context<RootView>,
    on_press: impl Fn() + 'static,
) -> Button {
    Button::new(id).small().child(label).on_click(cx.listener(
        move |_: &mut RootView, _: &ClickEvent, _: &mut Window, cx| {
            on_press();
            cx.notify();
        },
    ))
}
