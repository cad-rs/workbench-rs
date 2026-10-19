//! Dropdown popups for Split arrows, Menu buttons and Combo boxes.
//!
//! Positioning follows docs/ribbon-bar.md section 4.4:
//! - the popup top edge sits 4px below the trigger bottom edge, always
//!   opening downward (no vertical flip; the 8px window-edge clamp applies
//!   instead);
//! - left-aligned to the trigger by default;
//! - flipped to right-aligned when the trigger is too close to the right
//!   window edge, with a final 8px window-edge clamp on both sides;
//! - at most one popup is open at a time (the state holds a single
//!   [`PopupHandle`]; opening a new one drops the old one).
//!
//! gpui-component's `PopupMenu` provides the entry chrome (icon / checked /
//! disabled / separators) and the dismiss behavior; the anchor computation
//! here is ribbon-specific because a bare anchor corner cannot express the
//! right-overflow flip rule of section 4.4.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Anchor, AnyElement, Bounds, ClickEvent, Edges, Entity, FocusHandle, IntoElement,
    ParentElement as _, Point, Pixels, Window, anchored, deferred, point, px,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::ElementExt as _;

use crate::model::{MenuEntry, RibbonEvent, RibbonItemKindVm, RibbonItemVm};
use crate::state::RibbonState;
use crate::tokens;

/// What a dropdown should contain when it opens.
#[derive(Debug, Clone)]
pub(crate) enum DropdownRequest {
    /// Split arrow / Menu button: plain command entries (each entry carries
    /// its own command id).
    Items { entries: Vec<MenuEntry> },
    /// Combo box: option list with the current option checked.
    Combo {
        command_id: String,
        current: String,
        options: Vec<String>,
    },
}

/// A live dropdown popup anchored to the trigger that opened it.
pub(crate) struct PopupHandle {
    menu: Entity<PopupMenu>,
    /// Window-space bounds of the trigger, captured at open time.
    pub(crate) trigger: Bounds<Pixels>,
    /// Focus to restore when the popup dismisses.
    pub(crate) restore_focus: Option<FocusHandle>,
    /// Keeps the DismissEvent subscription alive while the popup is open.
    _subscription: gpui::Subscription,
}

impl PopupHandle {
    pub(crate) fn new(
        menu: Entity<PopupMenu>,
        trigger: Bounds<Pixels>,
        restore_focus: Option<FocusHandle>,
        subscription: gpui::Subscription,
    ) -> Self {
        Self {
            menu,
            trigger,
            restore_focus,
            _subscription: subscription,
        }
    }
}

/// Shared cell that records the window-space bounds of a trigger element
/// during prepaint; the click handler reads it back to anchor the popup.
pub(crate) type TriggerBoundsCell = Rc<Cell<Option<Bounds<Pixels>>>>;

pub(crate) fn new_trigger_cell() -> TriggerBoundsCell {
    Rc::new(Cell::new(None))
}

/// Wraps an element so its window-space bounds are recorded into `cell`
/// during prepaint.
pub(crate) fn track_trigger<E: gpui::ParentElement + gpui::IntoElement>(
    element: E,
    cell: &TriggerBoundsCell,
) -> E {
    let cell = cell.clone();
    element.on_prepaint(move |bounds, _, _| cell.set(Some(bounds)))
}

/// Builds the `PopupMenu` entity for `request`, wiring each entry to emit a
/// [`RibbonEvent::Command`].
pub(crate) fn build_menu(
    request: &DropdownRequest,
    window: &mut Window,
    cx: &mut gpui::Context<RibbonState>,
) -> Entity<PopupMenu> {
    let max_w = (window.viewport_size().width - px(tokens::POPUP_EDGE_MARGIN * 2.0))
        .max(px(tokens::POPUP_MIN_WIDTH));

    let items: Vec<PopupMenuItem> = match request {
        DropdownRequest::Items { entries, .. } => entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Separator => PopupMenuItem::separator(),
                MenuEntry::Item(vm) => {
                    let entry_id = vm.id.clone();
                    let handler = cx.listener(
                        move |this: &mut RibbonState,
                              _: &ClickEvent,
                              window: &mut Window,
                              cx: &mut gpui::Context<RibbonState>| {
                            this.emit(
                                RibbonEvent::Command {
                                    id: entry_id.clone(),
                                    value: None,
                                },
                                window,
                                cx,
                            );
                        },
                    );
                    let mut item = PopupMenuItem::new(vm.label.clone())
                        .disabled(!vm.enabled)
                        .checked(vm.checked)
                        .on_click(handler);
                    if let Some(icon) = &vm.icon {
                        item = item.icon(gpui_component::Icon::empty().path(icon.clone()));
                    }
                    item
                }
            })
            .collect(),
        DropdownRequest::Combo {
            command_id,
            current,
            options,
        } => options
            .iter()
            .map(|option| {
                let command_id = command_id.clone();
                let option = option.clone();
                let is_current = *option == *current;
                let option_label = option.clone();
                let handler = cx.listener(
                    move |this: &mut RibbonState,
                          _: &ClickEvent,
                          window: &mut Window,
                          cx: &mut gpui::Context<RibbonState>| {
                        this.emit(
                            RibbonEvent::Command {
                                id: command_id.clone(),
                                value: Some(option.clone()),
                            },
                            window,
                            cx,
                        );
                    },
                );
                PopupMenuItem::new(option_label)
                    .checked(is_current)
                    .on_click(handler)
            })
            .collect(),
    };

    PopupMenu::build(window, cx, move |menu, _, _| {
        let mut menu = menu
            .min_w(px(tokens::POPUP_MIN_WIDTH))
            .max_w(max_w);
        for item in items {
            menu = menu.item(item);
        }
        menu
    })
}

/// The dropdown entries declared by a Split / Menu item, if any.
pub(crate) fn item_entries(item: &RibbonItemVm) -> Option<Vec<MenuEntry>> {
    match &item.kind {
        RibbonItemKindVm::Split { items } | RibbonItemKindVm::Menu { items } => Some(items.clone()),
        _ => None,
    }
}

/// Renders the open popup as a deferred anchored overlay above everything else
/// in the ribbon.
pub(crate) fn render_popup(handle: &PopupHandle, window: &Window) -> AnyElement {
    let viewport_w = window.viewport_size().width;
    let gap = px(tokens::POPUP_GAP);
    let edge = px(tokens::POPUP_EDGE_MARGIN);

    // Right overflow flip: when trigger-left plus the minimal popup width
    // would cross the right content edge, right-align to the trigger instead
    // (popup right edge == trigger right edge), then the 8px snap margin
    // clamps whatever still overflows.
    let flip_right = handle.trigger.origin.x + px(tokens::POPUP_MIN_WIDTH) > viewport_w - edge;
    let position: Point<Pixels> = if flip_right {
        point(handle.trigger.right(), handle.trigger.bottom() + gap)
    } else {
        point(handle.trigger.origin.x, handle.trigger.bottom() + gap)
    };
    let anchor = if flip_right {
        Anchor::TopRight
    } else {
        Anchor::TopLeft
    };

    deferred(
        anchored()
            .position(position)
            .anchor(anchor)
            // Clamp instead of switching anchors: downward-only expansion is a
            // section 4.4 invariant (the ribbon lives at the window top).
            .snap_to_window_with_margin(Edges::all(edge))
            .child(handle.menu.clone()),
    )
    // Same paint tier as gpui-component popup surfaces.
    .priority(100usize)
    .into_any_element()
}
