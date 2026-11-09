//! The in-app file picker overlay: open and save dialogs for storage without
//! an OS dialog (the browser's private file system, cloud stores).
//!
//! The runtime owns the state (`ws.file_picker`) and lists folders; this
//! module draws it and reports the user's moves — navigation through
//! [`workbench_core::file_picker`], the choice through
//! `AppServices::finish_file_dialog`. A save picker's file name is a
//! gpui-component `Input` the root view mounts while the picker is open.

use gpui::{
    AnyElement, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use gpui::prelude::FluentBuilder as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Escape, Input, InputEvent, InputState};
use gpui_component::{Icon, Sizable as _};
use workbench_api::{self as wb, AppServices as _};
use workbench_core::file_picker;

use crate::{RootView, SharedCore, col, theme};

/// Whether the picker is open (the root view suppresses hotkeys meanwhile).
pub(crate) fn is_open(core: &SharedCore) -> bool {
    lock(core).host.ws.file_picker.is_some()
}

/// The save picker's suggested file name, when a save picker is open.
pub(crate) fn save_name(core: &SharedCore) -> Option<String> {
    lock(core).host.ws.file_picker.as_ref()?.save.clone()
}

/// The file-name input for a save picker, seeded and focused; Enter saves.
pub(crate) fn create_name_input(
    core: &SharedCore,
    seed: &str,
    window: &mut Window,
    cx: &mut Context<RootView>,
) -> (Entity<InputState>, Subscription) {
    let input = crate::text_edit::create_input(seed, window, cx);
    let core = core.clone();
    let sub = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| match event {
        InputEvent::PressEnter { .. } => {
            save(&core, &input.read(cx).value());
            cx.notify();
        }
        // The save button turns into "Overwrite" as the name meets an existing file.
        InputEvent::Change => cx.notify(),
        _ => {}
    });
    (input, sub)
}

/// The picker overlay, when the picker is open.
pub(crate) fn overlay(
    core: &SharedCore,
    name_input: Option<&Entity<InputState>>,
    cx: &mut Context<RootView>,
) -> Option<AnyElement> {
    let typed = name_input.map(|input| input.read(cx).value().to_string());
    let snap = {
        let guard = lock(core);
        let picker = guard.host.ws.file_picker.as_ref()?;
        let host = &guard.host;
        // A typed name that is an existing file overwrites it.
        let overwrite = typed
            .as_deref()
            .is_some_and(|name| picker.existing(&picker.file_name(name)).is_some());
        Snap {
            title: host.tr(if picker.save.is_some() { "Save Document" } else { "Open Document" }),
            // Source labels (and the root crumb, which is one) are UI text;
            // folder names are the user's and stay as they are.
            sources: picker
                .sources
                .iter()
                .map(|(id, label)| (id.clone(), host.tr(label)))
                .collect(),
            provider: picker.provider.clone(),
            trail: picker
                .trail
                .iter()
                .enumerate()
                .map(|(depth, (_, name))| if depth == 0 { host.tr(name) } else { name.clone() })
                .collect(),
            listing: picker.listing.clone(),
            new_folder: host.tr("New Folder"),
            cancel: host.tr("Cancel"),
            confirm: host.tr(if overwrite { "Overwrite" } else { "Save" }),
            loading: host.tr("Loading…"),
            empty: host.tr("(empty)"),
            open_hint: host.tr("Click a file to open it"),
        }
    };
    Some(render(snap, core, name_input, cx).into_any_element())
}

/// Everything the overlay draws, snapshotted under the runtime lock.
struct Snap {
    title: String,
    sources: Vec<(String, String)>,
    provider: String,
    trail: Vec<String>,
    listing: Option<Result<Vec<wb::Entry>, String>>,
    new_folder: String,
    cancel: String,
    /// The save button: "Save", or "Overwrite" over an existing file.
    confirm: String,
    loading: String,
    empty: String,
    open_hint: String,
}

fn lock(core: &SharedCore) -> std::sync::MutexGuard<'_, workbench_core::AppRuntime> {
    core.lock().unwrap_or_else(|p| p.into_inner())
}

fn navigate(core: &SharedCore, step: impl FnOnce(&mut wb::FilePicker)) {
    file_picker::navigate(&mut wb::CommandCtx::new(&mut *lock(core)), step);
}

fn pick(core: &SharedCore, picked: Option<wb::Location>) {
    lock(core).finish_file_dialog(picked);
}

/// Save under the typed `name` in the folder shown (a blank name does
/// nothing).
fn save(core: &SharedCore, name: &str) {
    let mut guard = lock(core);
    let target = guard.host.ws.file_picker.as_ref().and_then(|picker| picker.target(name));
    if let Some(target) = target {
        guard.finish_file_dialog(Some(target));
    }
}

fn render(
    snap: Snap,
    core: &SharedCore,
    name_input: Option<&Entity<InputState>>,
    cx: &mut Context<RootView>,
) -> gpui::Stateful<gpui::Div> {
    let theme = theme::ui_theme();
    let muted = col(wb::Color::GRAY);
    let hover = col(theme.toolbar);

    // Backdrop: a click outside the card or Escape cancels.
    let backdrop = div()
        .id("file-picker-backdrop")
        .occlude()
        .absolute()
        .size_full()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(80.0))
        .bg(gpui::rgba(0x000000_30))
        .on_click(cx.listener({
            let core = core.clone();
            move |_, _: &ClickEvent, _, cx| {
                pick(&core, None);
                cx.notify();
            }
        }))
        .on_action(cx.listener({
            let core = core.clone();
            move |_, _: &Escape, _, cx| {
                pick(&core, None);
                cx.notify();
            }
        }));

    // Places to pick from.
    let mut sources = div().flex().flex_col().w(px(132.0)).gap(px(2.0));
    for (i, (id, label)) in snap.sources.iter().enumerate() {
        let selected = *id == snap.provider;
        let core = core.clone();
        let id = id.clone();
        sources = sources.child(
            div()
                .id(("file-picker-source", i))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .when(selected, |row| row.bg(col(theme.toolbar)))
                .hover(|row| row.bg(hover))
                .child(SharedString::from(label.clone()))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    navigate(&core, |picker| picker.browse(&id));
                    cx.notify();
                })),
        );
    }

    // Breadcrumbs: each folder up the trail is a way back.
    let mut crumbs = div().flex().flex_1().items_center().gap(px(4.0)).text_size(px(12.0));
    let last = snap.trail.len().saturating_sub(1);
    for (depth, name) in snap.trail.iter().enumerate() {
        if depth > 0 {
            crumbs = crumbs.child(div().text_color(muted).child("/"));
        }
        let core = core.clone();
        crumbs = crumbs.child(
            div()
                .id(("file-picker-crumb", depth))
                .px(px(4.0))
                .rounded(px(3.0))
                .when(depth < last, |crumb| crumb.hover(|c| c.bg(hover)))
                .child(SharedString::from(name.clone()))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    navigate(&core, |picker| picker.up_to(depth));
                    cx.notify();
                })),
        );
    }
    let new_folder = Button::new("file-picker-new-folder")
        .ghost()
        .small()
        .child(snap.new_folder.clone())
        .on_click(cx.listener({
            let core = core.clone();
            let caption = snap.new_folder.clone();
            move |_, _: &ClickEvent, _, cx| {
                lock(&core).host.ws.text_edit = Some(wb::TextEdit::prompt(
                    "file_picker.new_folder",
                    "",
                    file_picker::NEW_FOLDER,
                    caption.clone(),
                ));
                cx.notify();
            }
        }));
    let toolbar = div().flex().items_center().gap(px(8.0)).child(crumbs).child(new_folder);

    // The folder shown.
    let mut rows = div()
        .id("file-picker-listing")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .rounded(px(4.0))
        .border_1()
        .border_color(col(theme.placeholder))
        .bg(col(theme.placeholder));
    match &snap.listing {
        None => rows = rows.child(note(&snap.loading, muted)),
        Some(Err(error)) => rows = rows.child(note(error, col(wb::Color::RED))),
        Some(Ok(entries)) if entries.is_empty() => rows = rows.child(note(&snap.empty, muted)),
        Some(Ok(entries)) => {
            for (i, entry) in entries.iter().enumerate() {
                rows = rows.child(entry_row(i, entry, &snap, core, name_input, cx));
            }
        }
    }

    // Footer: the file name (save) and the buttons.
    let mut footer = div().flex().items_center().gap(px(8.0));
    match name_input {
        Some(input) => {
            footer = footer.child(
                div()
                    .flex_1()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(col(theme.placeholder))
                    .child(Input::new(input)),
            );
            let core = core.clone();
            let input = input.clone();
            footer = footer.child(
                Button::new("file-picker-save")
                    .primary()
                    .small()
                    .child(snap.confirm.clone())
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        save(&core, &input.read(cx).value());
                        cx.notify();
                    })),
            );
        }
        None => {
            footer = footer.child(
                div()
                    .flex_1()
                    .text_size(px(11.0))
                    .text_color(muted)
                    .child(snap.open_hint.clone()),
            );
        }
    }
    footer = footer.child(
        Button::new("file-picker-cancel")
            .outline()
            .small()
            .child(snap.cancel.clone())
            .on_click(cx.listener({
                let core = core.clone();
                move |_, _: &ClickEvent, _, cx| {
                    pick(&core, None);
                    cx.notify();
                }
            })),
    );

    let folder = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(6.0))
        .child(toolbar)
        .child(rows);
    let card = div()
        .id("file-picker-card")
        .flex()
        .flex_col()
        .w(px(640.0))
        .h(px(420.0))
        .p(px(12.0))
        .gap(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(col(theme.placeholder))
        .bg(col(theme.bg))
        .text_color(col(wb::Color::LIGHT_GRAY))
        .text_size(px(13.0))
        // Swallow clicks so clicking the card does not cancel.
        .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.stop_propagation()))
        .child(div().text_size(px(14.0)).child(snap.title.clone()))
        .child(
            div()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .gap(px(8.0))
                .when(snap.sources.len() > 1, |body| body.child(sources))
                .child(folder),
        )
        .child(footer);
    backdrop.child(card)
}

/// One folder or file of the listing: a folder opens on click; a file is
/// picked (open) or names the file to save (save).
fn entry_row(
    i: usize,
    entry: &wb::Entry,
    snap: &Snap,
    core: &SharedCore,
    name_input: Option<&Entity<InputState>>,
    cx: &mut Context<RootView>,
) -> gpui::Stateful<gpui::Div> {
    let icon = if entry.is_dir { "icons/folder.svg" } else { "icons/file.svg" };
    let row = div()
        .id(("file-picker-entry", i))
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .h(px(26.0))
        .flex_shrink_0()
        .hover(|row| row.bg(col(theme::ui_theme().toolbar)))
        .child(Icon::empty().path(icon).small().text_color(col(wb::Color::GRAY)))
        .child(SharedString::from(entry.name.clone()));
    let core = core.clone();
    let entry = entry.clone();
    let location = wb::Location::new(snap.provider.as_str(), entry.key.as_str());
    let name_input = name_input.cloned();
    row.on_click(cx.listener(move |_, _: &ClickEvent, window, cx| {
        if entry.is_dir {
            navigate(&core, |picker| picker.enter(&entry));
        } else if let Some(input) = &name_input {
            input.update(cx, |state, cx| state.set_value(entry.name.clone(), window, cx));
        } else {
            pick(&core, Some(location.clone()));
        }
        cx.notify();
    }))
}

fn note(text: &str, color: gpui::Hsla) -> gpui::Div {
    div().p(px(12.0)).text_color(color).child(text.to_string())
}

