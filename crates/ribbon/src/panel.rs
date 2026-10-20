//! Panel area: the active tab's groups laid out in columns, with the
//! horizontal-scroll overflow strategy (docs/ribbon-bar.md sections 2.2-2.3,
//! 4.5-A) and the five item kinds (section 2.4).

use gpui::ColorExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement, Styled as _, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::separator::Separator;
use gpui_component::{Disableable as _, ElementExt as _, Icon, IconName, Sizable as _, Size, h_flex, v_flex};

use crate::layout::{self, ColumnPlan};
use crate::model::{
    ItemSize, MenuEntry, OverflowStrategy, RibbonEvent, RibbonGroupVm, RibbonItemKindVm,
    RibbonItemVm,
};
use crate::popup::{self, DropdownRequest, TriggerBoundsCell};
use crate::state::RibbonState;
use crate::tabs::{button_variant, icon_color, placeholder_glyph};
use crate::tokens;

/// Renders the 86px panel area showing the active tab's groups.
pub(crate) fn render_panel(state: &RibbonState, cx: &mut Context<RibbonState>) -> AnyElement {
    match state.model.overflow {
        // Group collapse overflow is not implemented yet; the scroll strategy
        // renders as the fallback.
        // TODO(ribbon): implement strategy B (group collapse overflow).
        OverflowStrategy::Scroll | OverflowStrategy::Collapse => render_scroll_panel(state, cx),
    }
}

fn render_scroll_panel(state: &RibbonState, cx: &mut Context<RibbonState>) -> AnyElement {
    let offset = state.clamped_scroll_offset();
    let viewport_w = state.viewport_width();
    let content_w = state.content_width();
    let scrollable = content_w > viewport_w + 0.5;

    let viewport_cell = state.panel_viewport_w.clone();
    let content_cell = state.panel_content_w.clone();
    let entity = cx.entity();
    let group_elements = group_children(state, cx);
    let left_arrow = scrollable
        .then(|| scroll_arrow(-1.0, true, offset > 0.0, cx));
    let right_arrow = scrollable
        .then(|| scroll_arrow(1.0, false, offset < content_w - viewport_w, cx));

    let content_entity = entity.clone();
    let content_row = h_flex()
        .items_stretch()
        .flex_shrink_0()
        .relative()
        .left(px(-offset))
        .on_prepaint(move |bounds, _, cx| {
            // Re-render on measurement change: the first pass measured zero,
            // so the scroll arrows would never appear without this repaint.
            let w = f32::from(bounds.size.width);
            if content_cell.get() != w {
                content_cell.set(w);
                cx.notify(content_entity.entity_id());
            }
        })
        .children(group_elements);

    div()
        .h(px(tokens::PANEL_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_stretch()
        .px(px(8.0))
        .bg(tokens::ribbon_bg())
        .on_prepaint(move |bounds, _, cx| {
            let w = f32::from(bounds.size.width);
            if viewport_cell.get() != w {
                viewport_cell.set(w);
                cx.notify(entity.entity_id());
            }
        })
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                // Flex container so the shrink-0 content row sizes to its
                // max-content width: as a plain block child it would fill
                // the viewport and the overflow measurement would never
                // exceed it (arrows could never appear).
                .flex()
                .on_scroll_wheel(cx.listener(RibbonState::handle_scroll_wheel))
                .child(content_row)
                .when_some(left_arrow, |viewport, arrow| viewport.child(arrow))
                .when_some(right_arrow, |viewport, arrow| viewport.child(arrow)),
        )
        .into_any_element()
}

/// All groups of the active tab, separated by 1px vertical lines.
fn group_children(state: &RibbonState, cx: &mut Context<RibbonState>) -> Vec<AnyElement> {
    let groups: &[RibbonGroupVm] = state
        .active_tab()
        .map(|tab| tab.groups.as_slice())
        .unwrap_or(&[]);

    let mut children = Vec::with_capacity(groups.len() * 2);
    for group in groups {
        if !children.is_empty() {
            children.push(
                Separator::vertical()
                    .color(tokens::group_sep())
                    .h(px(
                        tokens::BUTTON_ZONE_HEIGHT
                            + tokens::LABEL_HEIGHT
                            + tokens::CHEVRON_STRIP_HEIGHT,
                    ))
                    .self_center()
                    .flex_shrink_0()
                    .into_any_element(),
            );
        }
        children.push(render_group(state, group, cx));
    }
    children
}

/// One group: item columns over a centered 10px caption.
fn render_group(
    state: &RibbonState,
    group: &RibbonGroupVm,
    cx: &mut Context<RibbonState>,
) -> AnyElement {
    let plans = layout::plan_columns(&group.items);

    v_flex()
        .h_full()
        .flex_shrink_0()
        .px(px(8.0))
        .py(px(4.0))
        .child(
            h_flex()
                .items_stretch()
                .h(px(
                    tokens::BUTTON_ZONE_HEIGHT
                        + tokens::LABEL_HEIGHT
                        + tokens::CHEVRON_STRIP_HEIGHT,
                ))
                .children(plans.iter().map(|plan| match plan {
                    ColumnPlan::Large(ix) => {
                        render_large_column(state, &group.items[*ix], &group.id, *ix, cx)
                    }
                    ColumnPlan::Stack(rows) => v_flex()
                        .items_start()
                        .children(rows.iter().map(|ix| {
                            render_row_item(state, &group.items[*ix], &group.id, *ix, cx)
                        }))
                        .into_any_element(),
                })),
        )
        .child(
            div()
                .h(px(tokens::GROUP_TITLE_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .text_color(tokens::text_secondary())
                .child(SharedString::from(group.title.clone())),
        )
        .into_any_element()
}

/// Stable, unique element id for a ribbon item: the same command may
/// legitimately back several items (a Split and a Menu, say), so group +
/// index disambiguate.
fn item_element_id(group_key: &str, item_ix: usize, command: &str) -> SharedString {
    SharedString::from(format!("panel-item-{group_key}/{item_ix}/{command}"))
}

/// Per-item click dispatch shared by the large and row button builders.
///
/// A Split's chevron hit zone routes to the dropdown branch, a Menu's whole
/// button opens the list, and everything else emits the default command.
/// The cells carry the whole-button bounds (dropdown anchor) and the chevron
/// rect (hit zone); the button builders feed them via `track_trigger`.
struct ItemChrome {
    item_id: String,
    entries: Option<Vec<MenuEntry>>,
    is_split: bool,
    is_menu: bool,
    checked: bool,
    anchor: TriggerBoundsCell,
    strip: TriggerBoundsCell,
}

impl ItemChrome {
    fn new(state: &RibbonState, item: &RibbonItemVm) -> Self {
        Self {
            item_id: item.id.clone(),
            entries: popup::item_entries(item),
            is_split: matches!(item.kind, RibbonItemKindVm::Split { .. }),
            is_menu: matches!(item.kind, RibbonItemKindVm::Menu { .. }),
            checked: matches!(&item.kind, RibbonItemKindVm::Toggle { checked } if *checked),
            anchor: state.new_popup_trigger_cell(),
            strip: state.new_popup_trigger_cell(),
        }
    }

    /// Whether this kind carries a chevron affordance (Split / Menu).
    fn shows_chevron(&self) -> bool {
        self.is_split || self.is_menu
    }

    /// Attaches the single position-dispatching click handler.
    fn attach_click(&self, button: Button, cx: &mut Context<RibbonState>) -> Button {
        let item_id = self.item_id.clone();
        let entries = self.entries.clone();
        let is_split = self.is_split;
        let is_menu = self.is_menu;
        let anchor = self.anchor.clone();
        let strip = self.strip.clone();
        button.on_click(cx.listener(
            move |this: &mut RibbonState, ev: &ClickEvent, window: &mut Window, cx| {
                let open_menu = |this: &mut RibbonState,
                                 window: &mut Window,
                                 cx: &mut Context<RibbonState>| {
                    if let (Some(bounds), Some(entries)) = (anchor.get(), entries.as_ref()) {
                        if !entries.is_empty() {
                            this.open_dropdown(
                                bounds,
                                DropdownRequest::Items {
                                    entries: entries.clone(),
                                },
                                window,
                                cx,
                            );
                        }
                    }
                };
                // Split strip hit zone routes to the dropdown branch.
                if is_split {
                    if let (Some(rect), Some(pos)) = (strip.get(), click_position(ev)) {
                        if rect.contains(&pos) {
                            open_menu(this, window, cx);
                            return;
                        }
                    }
                }
                if is_menu {
                    open_menu(this, window, cx);
                    return;
                }
                this.emit(
                    RibbonEvent::Command {
                        id: item_id.clone(),
                        value: None,
                    },
                    window,
                    cx,
                );
            },
        ))
    }
}

/// Pointer position of a click (mouse / tap / keyboard-activated button).
fn click_position(ev: &ClickEvent) -> Option<gpui::Point<gpui::Pixels>> {
    match ev {
        ClickEvent::Mouse(click) => Some(click.up.position),
        ClickEvent::Touch(tap) => Some(tap.position),
        ClickEvent::Keyboard(kb) => Some(kb.bounds.center()),
    }
}

/// A Large item column: 48px wide, icon zone over a two-line label. All
/// kinds share ONE structure and ONE click handler; a Split adds a corner
/// chevron affordance whose hit zone is resolved by click position, so the
/// presence of a chevron never changes the icon/label layout. A Combo
/// declared Large renders as a centered row item instead. Element ids are
/// keyed by group + item index + command: the same command may legitimately
/// back several items (a Split and a Menu, say), and duplicate ids crash
/// gpui's a11y tree in dev builds.
fn render_large_column(
    state: &RibbonState,
    item: &RibbonItemVm,
    group_key: &str,
    item_ix: usize,
    cx: &mut Context<RibbonState>,
) -> AnyElement {
    if matches!(item.kind, RibbonItemKindVm::Combo { .. }) {
        return div()
            .w(px(tokens::LARGE_COLUMN_WIDTH))
            .flex()
            .items_center()
            .child(render_row_item(state, item, group_key, item_ix, cx))
            .into_any_element();
    }

    let element_id = item_element_id(group_key, item_ix, &item.id);
    let chrome = ItemChrome::new(state, item);

    let icon_zone = div()
        .h(px(tokens::BUTTON_ZONE_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .child(large_icon(item));

    // 6 o'clock chevron strip: below the label, centered, in flow. Every
    // large button reserves this 12px row — split/menu render the chevron
    // there, other kinds leave it empty — so icons and labels sit at
    // identical y positions whether or not a chevron is present. The strip
    // rect feeds the Split click dispatch (strip hit zone = menu).
    let strip_row = if chrome.is_split {
        let strip = div()
            .id(SharedString::from(format!("{element_id}-arrow")))
            .h(px(tokens::CHEVRON_STRIP_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(2.0))
            .when(item.enabled, |d| {
                d.cursor_pointer().hover(|s| s.bg(tokens::btn_hover()))
            })
            .text_color(icon_color(item.enabled))
            .child(
                Icon::new(IconName::ChevronDown).with_size(Size::Size(px(10.0))),
            );
        popup::track_trigger(strip, &chrome.strip).into_any_element()
    } else if chrome.is_menu {
        div()
            .h(px(tokens::CHEVRON_STRIP_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(icon_color(item.enabled))
            .child(
                Icon::new(IconName::ChevronDown).with_size(Size::Size(px(10.0))),
            )
            .into_any_element()
    } else {
        div().h(px(tokens::CHEVRON_STRIP_HEIGHT)).into_any_element()
    };

    let mut button = base_button(element_id, item, chrome.checked, cx)
        .w(px(tokens::LARGE_COLUMN_WIDTH))
        .h(px(
            tokens::BUTTON_ZONE_HEIGHT
                + tokens::LABEL_HEIGHT
                + tokens::CHEVRON_STRIP_HEIGHT,
        ))
        .flex_shrink_0()
        .child(
            v_flex()
                .size_full()
                .child(icon_zone)
                .child(large_label(item))
                .child(strip_row),
        );

    if chrome.shows_chevron() {
        button = popup::track_trigger(button, &chrome.anchor);
    }
    chrome.attach_click(button, cx).into_any_element()
}

/// A 22px row item: Small / Icon button, Split, Menu, or Combo. Non-combo
/// kinds share ONE structure and ONE click handler — a Split's corner hit
/// zone is resolved by click position and a Menu's whole button opens the
/// list, so the presence of a chevron never changes the icon/label layout.
fn render_row_item(
    state: &RibbonState,
    item: &RibbonItemVm,
    group_key: &str,
    item_ix: usize,
    cx: &mut Context<RibbonState>,
) -> AnyElement {
    let element_id = item_element_id(group_key, item_ix, &item.id);
    if matches!(item.kind, RibbonItemKindVm::Combo { .. }) {
        return render_combo(state, item, element_id, cx);
    }

    let chrome = ItemChrome::new(state, item);

    let mut button = base_button(element_id, item, chrome.checked, cx)
        // Fixed 3-slot grid row: 86/3 each, top-down; columns with fewer
        // items leave the lower slots empty (no 2-way division).
        .h(px(tokens::ROW_HEIGHT))
        .min_w_0()
        .px(px(2.0))
        .gap(px(2.0))
        .child(row_content(item, chrome.shows_chevron(), chrome.is_split.then_some(&chrome.strip)));

    if chrome.shows_chevron() {
        button = popup::track_trigger(button, &chrome.anchor);
    }
    chrome.attach_click(button, cx).into_any_element()
}

/// Combo box row: value area + chevron; the whole box opens the option list.
fn render_combo(
    state: &RibbonState,
    item: &RibbonItemVm,
    element_id: SharedString,
    cx: &mut Context<RibbonState>,
) -> AnyElement {
    let RibbonItemKindVm::Combo { current, options } = &item.kind else {
        return div().into_any_element();
    };
    let width = tokens::combo_width(item.width);
    let trigger = state.new_popup_trigger_cell();
    let mut button = base_button(element_id, item, false, cx)
        .w(px(width))
        .h(px(tokens::ROW_HEIGHT))
        .flex_shrink_0()
        .self_center()
        .border_1()
        .border_color(tokens::group_sep())
        .child(
            h_flex()
                .w_full()
                .h_full()
                .items_center()
                .justify_between()
                .px(px(4.0))
                .gap(px(2.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(icon_color(item.enabled))
                        .child(SharedString::from(current.clone())),
                )
                .child(
                    Icon::new(IconName::ChevronDown)
                        .with_size(Size::Size(px(12.0)))
                        .text_color(icon_color(item.enabled)),
                ),
        );
    button = with_trigger_tracking(button, &trigger);
    let current = current.clone();
    let options = options.clone();
    let combo_id = item.id.clone();
    button = button.on_click(cx.listener(
        move |this: &mut RibbonState, _: &ClickEvent, window: &mut Window, cx| {
            // The combo list anchors to the value box; disabled combos
            // never receive clicks (the Button swallows them).
            if let Some(bounds) = trigger.get() {
                this.open_dropdown(
                    bounds,
                    DropdownRequest::Combo {
                        command_id: combo_id.clone(),
                        current: current.clone(),
                        options: options.clone(),
                    },
                    window,
                    cx,
                );
            }
        },
    ));
    button.into_any_element()
}

// -- Shared button building blocks ------------------------------------------

/// Ribbon button base: custom color variant (transparent rest / hover /
/// pressed; accent 25% background plus accent outline when checked), rounded
/// 3px, tooltip; disabled buttons gray out and do not respond.
fn base_button(
    id: SharedString,
    item: &RibbonItemVm,
    checked: bool,
    cx: &mut Context<RibbonState>,
) -> gpui_component::button::Button {
    Button::new(id)
        .custom(button_variant(checked, cx))
        .outline()
        .rounded(px(3.0))
        .px(px(0.0))
        .py(px(0.0))
        .disabled(!item.enabled)
        .tooltip(SharedString::from(item.label.clone()))
}

/// Records the button bounds for dropdown anchoring.
fn with_trigger_tracking<E: gpui::ParentElement + gpui::IntoElement>(
    element: E,
    trigger: &TriggerBoundsCell,
) -> E {
    popup::track_trigger(element, trigger)
}

/// Icon zone content of a Large item: 24px icon, silently missing when the
/// resource is absent (the label still shows).
fn large_icon(item: &RibbonItemVm) -> AnyElement {
    match &item.icon {
        Some(path) => Icon::empty().path(path.clone())
            .with_size(Size::Size(px(tokens::ICON_LARGE)))
            .text_color(icon_color(item.enabled))
            .into_any_element(),
        None => div().into_any_element(),
    }
}

/// Two-line label strip (11px text, 13px line height) under a Large item
/// zone: centered, wraps to at most two lines, a third line clips. 4px
/// side margins keep adjacent columns' labels from running together.
fn large_label(item: &RibbonItemVm) -> AnyElement {
    div()
        .h(px(tokens::LABEL_HEIGHT))
        .w_full()
        .flex()
        .justify_center()
        .overflow_hidden()
        .px(px(4.0))
        .child(
            div()
                .w_full()
                .whitespace_normal()
                .line_height(px(tokens::LABEL_LINE_HEIGHT))
                .text_align(gpui::TextAlign::Center)
                .text_size(px(11.0))
                .text_color(icon_color(item.enabled))
                .child(SharedString::from(item.label.clone())),
        )
        .into_any_element()
}

/// Row content for Small / Icon tier buttons: 16px icon beside (Small) or
/// instead of (Icon) the 11.5px label, optional trailing chevron. The
/// chevron is visual only; its rect feeds the row's click dispatch when
/// `corner` is provided (Split).
fn row_content(
    item: &RibbonItemVm,
    show_chevron: bool,
    corner: Option<&TriggerBoundsCell>,
) -> AnyElement {
    let icon_tier = item.size == ItemSize::Icon;

    // 16px icon; the Icon tier falls back to a letter glyph when the
    // resource is absent, other tiers silently show no icon.
    let icon = match &item.icon {
        Some(path) => Icon::empty()
            .path(path.clone())
            .with_size(Size::Size(px(tokens::ICON_SMALL)))
            .text_color(icon_color(item.enabled))
            .into_any_element(),
        None if icon_tier => {
            placeholder_glyph(&item.label, icon_color(item.enabled)).into_any_element()
        }
        None => div().into_any_element(),
    };

    h_flex()
        .w_full()
        .h_full()
        .items_center()
        .gap(px(4.0))
        .child(icon)
        .when(!icon_tier, |row| {
            row.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(icon_color(item.enabled))
                    .child(SharedString::from(item.label.clone())),
            )
        })
        .when(show_chevron, |row| {
            let chevron = div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(
                    Icon::new(IconName::ChevronDown)
                        .with_size(Size::Size(px(10.0)))
                        .text_color(icon_color(item.enabled)),
                );
            let chevron = match corner {
                Some(cell) => popup::track_trigger(chevron, cell),
                None => chevron,
            };
            row.child(chevron.into_any_element())
        })
        .into_any_element()
}

/// Overlay arrow at a panel end: scrolls by ~80% of the viewport per click.
fn scroll_arrow(
    direction: f32,
    at_left: bool,
    enabled: bool,
    cx: &mut Context<RibbonState>,
) -> AnyElement {
    let id = if at_left {
        "ribbon-scroll-left"
    } else {
        "ribbon-scroll-right"
    };
    let chevron = if at_left {
        IconName::ChevronLeft
    } else {
        IconName::ChevronRight
    };

    div()
        .id(id)
        .absolute()
        .top_0()
        .bottom_0()
        .when(at_left, |arrow| arrow.left_0())
        .when(!at_left, |arrow| arrow.right_0())
        .w(px(tokens::SCROLL_ARROW_WIDTH))
        .flex()
        .items_center()
        .justify_center()
        .bg(tokens::ribbon_bg().opacity(0.7))
        .when(enabled, |arrow| {
            arrow
                .cursor_pointer()
                .hover(|style| style.bg(tokens::btn_hover().opacity(0.9)))
                .on_click(cx.listener(
                    move |this: &mut RibbonState,
                          _: &ClickEvent,
                          window: &mut Window,
                          cx: &mut Context<RibbonState>| {
                        this.scroll_by_page(direction, window, cx);
                    },
                ))
        })
        .child(
            Icon::new(chevron)
                .with_size(Size::Size(px(12.0)))
                .text_color(if enabled {
                    tokens::text()
                } else {
                    tokens::text_disabled()
                }),
        )
        .into_any_element()
}
