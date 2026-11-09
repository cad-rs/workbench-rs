//! The shell of the modal overlays, and the platform's dialogs (About,
//! Settings, Commands & Shortcuts), one open at a time in `ws.dialog`.

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FocusHandle, KeyDownEvent, Stateful, Window, div,
    prelude::*, px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::Sizable as _;
use gpui_component::scroll::ScrollableElement as _;
use workbench_api as wb;

use crate::{RootView, SharedCore, col, lock, theme};

/// The open platform dialog, while there is one.
pub(crate) fn overlay(
    core: &SharedCore,
    focus: &mut DialogFocus,
    window: &mut Window,
    cx: &mut Context<RootView>,
) -> Option<AnyElement> {
    let (open, on_top) = {
        let guard = lock(core);
        let ws = &guard.host.ws;
        (ws.dialog, ws.text_edit.is_none() && ws.file_picker.is_none())
    };
    focus.sync(open.is_some(), on_top, window, cx);
    Some(match open? {
        wb::PlatformDialog::About => crate::about::overlay(core, focus, cx),
        wb::PlatformDialog::Settings => crate::settings::overlay(core, focus, cx),
        wb::PlatformDialog::Shortcuts => crate::shortcuts::overlay(core, focus, cx),
        wb::PlatformDialog::UnsavedChanges => crate::unsaved::overlay(core, focus, cx),
    })
}

pub(crate) fn close(core: &SharedCore) {
    lock(core).host.ws.dialog = None;
}

/// A dimmed backdrop over the window and an empty card centered on it; the
/// caller fills the card and puts it in the backdrop. Clicking the backdrop
/// calls `close`, and clicks on the card stay on the card.
pub(crate) fn centered(
    id: &str,
    width: f32,
    close: impl Fn() + 'static,
    cx: &mut Context<RootView>,
) -> (Stateful<Div>, Stateful<Div>) {
    let theme = theme::ui_theme();
    let edge = theme.placeholder;
    let backdrop = div()
        .id(format!("{id}-backdrop"))
        .occlude()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x000000_40))
        .on_click(cx.listener(move |_: &mut RootView, _: &ClickEvent, _: &mut Window, cx| {
            close();
            cx.notify();
        }));
    let card = div()
        .id(format!("{id}-card"))
        .flex()
        .flex_col()
        .w(px(width))
        .rounded(px(8.0))
        .border_1()
        .border_color(col(wb::Color::rgb(edge.r * 0.4, edge.g * 0.4, edge.b * 0.4)))
        .bg(col(theme.bg))
        .text_color(col(wb::Color::LIGHT_GRAY))
        .on_click(cx.listener(|_: &mut RootView, _: &ClickEvent, _: &mut Window, cx| {
            cx.stop_propagation();
        }));
    (backdrop, card)
}

/// [`centered`], for a platform dialog: it holds the keyboard, and the
/// backdrop or Escape closes it.
pub(crate) fn platform_shell(
    id: &str,
    width: f32,
    core: &SharedCore,
    focus: &DialogFocus,
    cx: &mut Context<RootView>,
) -> (Stateful<Div>, Stateful<Div>) {
    let core_backdrop = core.clone();
    let (backdrop, card) = centered(id, width, move || close(&core_backdrop), cx);
    let core = core.clone();
    let backdrop = backdrop.track_focus(&focus.handle).on_key_down(cx.listener(
        move |_: &mut RootView, event: &KeyDownEvent, _: &mut Window, cx| {
            if event.keystroke.key == "escape" {
                close(&core);
                cx.notify();
                cx.stop_propagation();
            }
        },
    ));
    (backdrop, card)
}

pub(crate) fn close_button(
    id: &str,
    label: String,
    core: &SharedCore,
    cx: &mut Context<RootView>,
) -> Button {
    let core = core.clone();
    Button::new(format!("{id}-close")).primary().small().child(label).on_click(cx.listener(
        move |_: &mut RootView, _: &ClickEvent, _: &mut Window, cx| {
            close(&core);
            cx.notify();
        },
    ))
}

/// A platform dialog of `rows` that scroll under a title, with a Close button.
pub(crate) fn list(
    id: &str,
    title: String,
    close_label: String,
    rows: Vec<Div>,
    core: &SharedCore,
    focus: &DialogFocus,
    cx: &mut Context<RootView>,
) -> AnyElement {
    let (backdrop, card) = platform_shell(id, 520.0, core, focus, cx);
    let card = card
        .h(px(480.0))
        .max_h(relative(0.85))
        .p(px(20.0))
        .gap(px(12.0))
        .child(
            div()
                .text_size(px(16.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div().flex_1().min_h_0().child(
                div().size_full().flex().flex_col().gap_1().children(rows).overflow_y_scrollbar(),
            ),
        )
        .child(div().flex().justify_end().child(close_button(id, close_label, core, cx)));
    backdrop.child(card).into_any_element()
}

/// A row of a list dialog: `label` on the left, what the caller adds on the right.
pub(crate) fn row(label: String) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_1()
        .py_0p5()
        .child(div().text_size(px(12.)).text_color(col(wb::Color::GRAY)).child(label))
}

/// The rounded background of a small value in a row: a button or a key.
pub(crate) fn chip() -> Div {
    div()
        .px_2()
        .py_0p5()
        .rounded(px(3.))
        .bg(col(theme::CONTROL_BG))
        .text_size(px(12.))
        .text_color(col(wb::Color::LIGHT_GRAY))
}

/// Keeps the keyboard on a dialog while it is open and hands it back after.
/// GPUI sends keys along the focus path, so Escape only reaches a focused
/// dialog.
pub(crate) struct DialogFocus {
    handle: FocusHandle,
    previous: Option<FocusHandle>,
    open: bool,
}

impl DialogFocus {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            handle: cx.focus_handle(),
            previous: None,
            open: false,
        }
    }

    /// `on_top` is false while an overlay above the dialog needs the keyboard;
    /// the dialog takes it back when that overlay closes.
    fn sync(&mut self, open: bool, on_top: bool, window: &mut Window, cx: &mut App) {
        if open != self.open {
            self.open = open;
            if open {
                self.previous = window.focused(cx);
            } else if let Some(previous) = self.previous.take() {
                window.focus(&previous, cx);
            }
        }
        if open && on_top && !self.handle.is_focused(window) {
            window.focus(&self.handle, cx);
        }
    }
}
