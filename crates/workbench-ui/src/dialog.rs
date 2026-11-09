//! The shell shared by the centered modal dialogs (About, Settings, Commands
//! & Shortcuts).

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FocusHandle, KeyDownEvent, Stateful, Window, div,
    prelude::*, px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::Sizable as _;
use gpui_component::scroll::ScrollableElement as _;
use workbench_api as wb;

use crate::{col, theme};

/// A dimmed backdrop over the window and an empty card centered on it; the
/// caller fills the card and puts it in the backdrop. Clicking the backdrop
/// calls `close`, and clicks on the card stay on the card.
pub(crate) fn centered(
    id: &str,
    width: f32,
    close: impl Fn() + 'static,
    cx: &mut Context<crate::RootView>,
) -> (Stateful<Div>, Stateful<Div>) {
    let theme = theme::ui_theme();
    let backdrop = div()
        .id(format!("{id}-backdrop"))
        .occlude()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x000000_40))
        .on_click(cx.listener(
            move |_: &mut crate::RootView, _: &ClickEvent, _: &mut Window, cx| {
                close();
                cx.notify();
            },
        ));
    let card = div()
        .id(format!("{id}-card"))
        .flex()
        .flex_col()
        .w(px(width))
        .rounded(px(8.0))
        .border_1()
        .border_color(col(dim(theme.placeholder, 0.4)))
        .bg(col(theme.bg))
        .text_color(col(wb::Color::LIGHT_GRAY))
        .on_click(cx.listener(
            |_: &mut crate::RootView, _: &ClickEvent, _: &mut Window, cx| {
                cx.stop_propagation();
            },
        ));
    (backdrop, card)
}

/// A dialog of `rows` that scroll under a title, with a Close button. Escape,
/// the button and the backdrop all call `close`.
pub(crate) fn list(
    id: &str,
    title: String,
    close_label: String,
    rows: Vec<Div>,
    focus: &DialogFocus,
    close: impl Fn() + Clone + 'static,
    cx: &mut Context<crate::RootView>,
) -> AnyElement {
    let (backdrop, card) = centered(id, 520.0, close.clone(), cx);
    let on_escape = close.clone();
    let backdrop = backdrop.track_focus(&focus.handle).on_key_down(cx.listener(
        move |_: &mut crate::RootView, event: &KeyDownEvent, _: &mut Window, cx| {
            if event.keystroke.key == "escape" {
                on_escape();
                cx.notify();
                cx.stop_propagation();
            }
        },
    ));
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
        .child(
            div().flex().justify_end().child(
                Button::new(format!("{id}-close"))
                    .primary()
                    .small()
                    .child(close_label)
                    .on_click(cx.listener(
                        move |_: &mut crate::RootView, _: &ClickEvent, _: &mut Window, cx| {
                            close();
                            cx.notify();
                        },
                    )),
            ),
        );
    backdrop.child(card).into_any_element()
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
    pub(crate) fn sync(&mut self, open: bool, on_top: bool, window: &mut Window, cx: &mut App) {
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

/// Dim a shell color toward black by `factor`.
fn dim(c: wb::Color, factor: f32) -> wb::Color {
    wb::Color::rgb(c.r * factor, c.g * factor, c.b * factor)
}
