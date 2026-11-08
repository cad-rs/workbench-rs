//! Text-edit overlay: hosts a gpui-component `Input` while `ws.text_edit` is
//! active — a settings value (started by `app.settings.edit_begin` from the
//! settings dialog's string rows) or a value prompt any command opens
//! (`wb::TextEdit::prompt`).
//!
//! The Input component provides caret/selection/IME/clipboard/undo; this
//! module owns the lifecycle: create the state entity on open (seeded from
//! the edit buffer), focus it, commit on `InputEvent::PressEnter` through the
//! edit's commit command (`app.settings.set` for settings), discard on the
//! `Escape` action (which the Input propagates to its parent) or the close
//! button. Hotkeys stay suppressed while an edit is open (see [`is_active`]).

use gpui::{
    AppContext as _, ClickEvent, Context, Div, Entity, InteractiveElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use gpui_component::button::Button;
use gpui_component::input::{Escape, InputEvent, InputState};
use gpui_component::Sizable as _;
use workbench_api as wb;

use crate::{SharedCore, col, theme};

/// Whether a text edit is currently open (the root view suppresses hotkeys
/// while true so typing a key that is also a shortcut does not fire the
/// shortcut).
pub(crate) fn is_active(core: &SharedCore) -> bool {
    core.lock()
        .unwrap_or_else(|p| p.into_inner())
        .host
        .ws
        .text_edit
        .is_some()
}

/// What the overlay needs to mount (or re-mount) the Input for the open edit.
pub(crate) struct EditMount {
    pub key: String,
    pub seed: String,
    /// The command Enter runs (with `key` + `value`).
    pub commit: String,
}

/// Snapshot the open edit, if any and not yet mounted under `mounted_key`.
pub(crate) fn mount_for(core: &SharedCore, mounted_key: Option<&str>) -> Option<EditMount> {
    let guard = core.lock().unwrap_or_else(|p| p.into_inner());
    let edit = guard.host.ws.text_edit.as_ref()?;
    if mounted_key == Some(edit.key.as_str()) {
        return None;
    }
    Some(EditMount {
        key: edit.key.clone(),
        seed: edit.buffer.clone(),
        commit: edit.commit_command().to_string(),
    })
}

/// The caption of the open edit (shown above the input).
pub(crate) fn caption(core: &SharedCore) -> String {
    core.lock()
        .unwrap_or_else(|p| p.into_inner())
        .host
        .ws
        .text_edit
        .as_ref()
        .map(|edit| edit.caption().to_string())
        .unwrap_or_default()
}

/// Close the open edit (Escape / close button / backdrop click).
pub(crate) fn close(core: &SharedCore) {
    if let Ok(mut guard) = core.lock() {
        guard.host.ws.text_edit = None;
    }
}

/// Create the Input state for a freshly opened edit, seeded with the current
/// value and focused.
pub(crate) fn create_input(
    seed: &str,
    window: &mut Window,
    cx: &mut Context<crate::RootView>,
) -> Entity<InputState> {
    let state = cx.new(|cx| InputState::new(window, cx).default_value(seed));
    state.update(cx, |state, cx| state.focus(window, cx));
    state
}

/// Commit the Input's current value through the edit's commit command and
/// close.
pub(crate) fn commit(
    core: &SharedCore,
    command: &str,
    key: &str,
    input: &Entity<InputState>,
    cx: &mut Context<'_, crate::RootView>,
) {
    let value = input.read(cx).value().to_string();
    if let Ok(mut guard) = core.lock() {
        let _ = guard.execute_command(
            command,
            &wb::CommandArgs::new()
                .with("key", key)
                .with("value", value),
        );
        guard.host.ws.text_edit = None;
    }
}

/// Subscribe to the Input's Enter event: commit and close.
pub(crate) fn subscribe_enter(
    core: SharedCore,
    input: Entity<InputState>,
    key: String,
    command: String,
    cx: &mut Context<crate::RootView>,
) -> Subscription {
    cx.subscribe(&input, move |_, input, ev: &InputEvent, cx| {
        if matches!(ev, InputEvent::PressEnter { .. }) {
            commit(&core, &command, &key, &input, cx);
            cx.notify();
        }
    })
}

/// Renders the edit overlay shell for the given open edit: a backdrop (click
/// discards), the caption, the seeded Input, and Enter/Esc hints plus a
/// close button. The caller mounts the actual `Input` element inside
/// `input_slot`.
pub(crate) fn overlay_shell(
    caption: &str,
    core: &SharedCore,
    input_slot: Div,
    cx: &mut Context<crate::RootView>,
) -> gpui::Stateful<Div> {
    let theme = theme::ui_theme();

    let core_bg = core.clone();
    let core_esc = core.clone();
    let overlay = div()
        .id("text-edit-backdrop")
        .absolute()
        .size_full()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(120.0))
        .bg(gpui::rgba(0x000000_30))
        .on_click(cx.listener(
            move |_: &mut crate::RootView, _: &ClickEvent, _window: &mut Window, cx: &mut Context<'_, crate::RootView>| {
                close(&core_bg);
                cx.notify();
            },
        ))
        // Escape propagates out of the Input; catch it here (and only here:
        // the backdrop click covers the pointer case).
        .on_action(cx.listener(
            move |_: &mut crate::RootView, _: &Escape, _window: &mut Window, cx: &mut Context<'_, crate::RootView>| {
                close(&core_esc);
                cx.notify();
            },
        ));

    let mut card = div()
        .id("text-edit-card")
        .flex()
        .flex_col()
        .w(px(420.0))
        .p(px(16.0))
        .gap(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(col(theme.placeholder))
        .bg(col(theme.bg))
        .text_color(col(wb::Color::LIGHT_GRAY))
        // Swallow clicks so clicking the card does not discard.
        .on_click(cx.listener(
            |_: &mut crate::RootView, _: &ClickEvent, _: &mut Window, cx: &mut Context<'_, crate::RootView>| {
                cx.stop_propagation();
            },
        ));

    card = card.child(
        div()
            .text_size(px(12.0))
            .text_color(col(wb::Color::GRAY))
            .child(SharedString::from(caption.to_string())),
    );

    card = card.child(input_slot);

    // Hints + close.
    let (hint, cancel) = {
        let guard = core.lock().unwrap_or_else(|p| p.into_inner());
        (guard.host.tr("Enter to save · Esc to cancel"), guard.host.tr("Cancel"))
    };
    let core_btn = core.clone();
    let footer = div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(11.0))
                .text_color(col(wb::Color::GRAY))
                .child(hint),
        )
        .child(
            Button::new("text-edit-close")
                .outline()
                .small()
                .child(cancel)
                .on_click(cx.listener(
                    move |_: &mut crate::RootView, _: &ClickEvent, _: &mut Window, cx: &mut Context<'_, crate::RootView>| {
                        close(&core_btn);
                        cx.notify();
                    },
                )),
        );
    card = card.child(footer);

    overlay.child(card)
}
