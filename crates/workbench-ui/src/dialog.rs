//! The shell shared by the centered modal dialogs (About, Settings).

use gpui::{
    ClickEvent, Context, Div, InteractiveElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
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

/// Dim a shell color toward black by `factor`.
fn dim(c: wb::Color, factor: f32) -> wb::Color {
    wb::Color::rgb(c.r * factor, c.g * factor, c.b * factor)
}
