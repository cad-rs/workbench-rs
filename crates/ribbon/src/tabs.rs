//! Tab strip: Logo, QAT, separator, tabs and the collapse button (30px tall).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::{Disableable as _, Icon, IconName, Sizable as _, Size};

use crate::model::{RibbonEvent, RibbonItemKindVm, RibbonItemVm, RibbonTabVm};
use crate::state::RibbonState;
use crate::tokens;

/// Renders the 30px tab strip row.
pub(crate) fn render_tab_strip(state: &RibbonState, cx: &mut Context<RibbonState>) -> AnyElement {
    let has_qat = !state.model.qat.is_empty();
    let logo = state.model.logo.clone();
    let collapsed = state.model.collapsed;

    let mut strip = gpui_component::h_flex()
        .h(px(tokens::TAB_STRIP_HEIGHT))
        .w_full()
        .items_center()
        .flex_shrink_0()
        .pl(px(8.0))
        .pr(px(8.0))
        .gap(px(6.0))
        .bg(tokens::tab_strip_bg());

    // Logo: static display in the first version (it becomes the application
    // menu entry point once the menu system lands). 20x20, 8px from the left
    // window edge.
    if let Some(logo) = logo {
        strip = strip.child(
            Icon::empty()
                .path(logo)
                .with_size(Size::Size(px(tokens::LOGO_SIZE)))
                .text_color(tokens::text())
                .flex_shrink_0(),
        );
    }

    // QAT: icon tier command buttons; the point of the QAT is that it stays
    // usable while the ribbon is collapsed.
    if has_qat {
        strip = strip.child(
            gpui_component::h_flex()
                .items_center()
                .gap(px(2.0))
                .flex_shrink_0()
                .children(
                    state
                        .model
                        .qat
                        .iter()
                        .enumerate()
                        .map(|(ix, item)| render_qat_button(ix, item, cx)),
                ),
        );

        // 1px vertical separator between the QAT area and the tabs.
        strip = strip.child(
            gpui_component::separator::Separator::vertical()
                .color(tokens::group_sep())
                .h(px(16.0))
                .mx(px(6.0))
                .flex_shrink_0(),
        );
    }

    // Tabs row: fills the remaining width; the active tab carries the 2px
    // accent indicator bar and white text. An unknown active id falls back to
    // the first tab (same recovery rule as the panel area).
    let active_tab_id = state.model.active_tab_id.clone();
    let active_index = state
        .model
        .tabs
        .iter()
        .position(|tab| tab.id == active_tab_id)
        .or(if state.model.tabs.is_empty() {
            None
        } else {
            Some(0)
        });

    strip = strip.child(
        gpui_component::h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_stretch()
            .overflow_hidden()
            .children(state.model.tabs.iter().enumerate().map(|(ix, tab)| {
                render_tab(ix, tab, Some(ix) == active_index, cx)
            })),
        // TODO(ribbon): horizontal scrolling for the tab strip itself when the
        // window is too narrow (low priority; Logo/QAT stay pinned left).
    );

    // Collapse toggle at the right end of the strip: rendered only when the
    // collapse feature is enabled (disabled by default; collapsing is also
    // available via double-clicking the active tab and Ctrl+F1).
    if state.model.collapse_enabled {
        let chevron = if collapsed {
            IconName::ChevronDown
        } else {
            IconName::ChevronUp
        };
        strip = strip.child(
            strip_button(
                SharedString::from("ribbon-collapse"),
                None,
                icon_element(Icon::new(chevron), true),
                cx,
            )
            .on_click(cx.listener(
                |this: &mut RibbonState, _: &ClickEvent, window: &mut Window, cx| {
                    // Collapsing dismisses any open dropdown.
                    this.open_popup = None;
                    this.emit(RibbonEvent::ToggleCollapse, window, cx);
                },
            )),
        );
    }

    strip.into_any_element()
}

/// A small square strip button (QAT commands and the collapse button):
/// 16px icon inside a 22px hit area, rounded 3px.
fn strip_button(
    id: SharedString,
    item: Option<&RibbonItemVm>,
    icon: AnyElement,
    cx: &mut Context<RibbonState>,
) -> gpui_component::button::Button {
    let enabled = item.map(|item| item.enabled).unwrap_or(true);
    let checked =
        matches!(item.map(|item| &item.kind), Some(RibbonItemKindVm::Toggle { checked }) if *checked);
    let tooltip = item.map(|item| item.label.clone()).unwrap_or_default();

    Button::new(id)
        .custom(button_variant(checked, cx))
        .outline()
        .rounded(px(3.0))
        .px(px(0.0))
        .py(px(0.0))
        .w(px(tokens::QAT_SIZE))
        .h(px(tokens::QAT_SIZE))
        .flex_shrink_0()
        .disabled(!enabled)
        .when(!tooltip.is_empty(), |button| {
            button.tooltip(SharedString::from(tooltip))
        })
        .child(icon)
}

fn render_qat_button(
    ix: usize,
    item: &RibbonItemVm,
    cx: &mut Context<RibbonState>,
) -> AnyElement {
    let color = icon_color(item.enabled);
    let icon = item
        .icon
        .as_ref()
        .map(|path| {
            Icon::empty()
                .path(path.clone())
                .with_size(Size::Size(px(tokens::ICON_SMALL)))
                .into_any_element()
        })
        // Missing icon on an Icon tier item: first character placeholder block
        // instead of a dead blank area.
        .unwrap_or_else(|| placeholder_glyph(&item.label, color));

    let element_id = SharedString::from(format!("ribbon-qat-{ix}-{}", item.id));

    let mut button = strip_button(element_id, Some(item), icon, cx);
    let item_id = item.id.clone();
    button = button.on_click(
        cx.listener(move |this: &mut RibbonState, _: &ClickEvent, window: &mut Window, cx| {
            this.emit(
                RibbonEvent::Command {
                    id: item_id.clone(),
                    value: None,
                },
                window,
                cx,
            );
        }),
    );
    button.into_any_element()
}

/// One tab page: 12px horizontal / 6px vertical padding, 12.5px text, 2px
/// accent indicator bar at the bottom when active. Double click on the active
/// tab toggles collapse (Fluent convention).
fn render_tab(
    ix: usize,
    tab: &RibbonTabVm,
    is_active: bool,
    cx: &mut Context<RibbonState>,
) -> AnyElement {
    let tab_id = tab.id.clone();
    let element_id = SharedString::from(format!("ribbon-tab-{ix}-{}", tab.id));
    let was_active = is_active;

    div()
        .id(element_id)
        .relative()
        .h_full()
        .px(px(12.0))
        .py(px(6.0))
        .flex()
        .items_center()
        .flex_shrink_0()
        .text_size(px(12.5))
        .text_color(if is_active {
            tokens::tab_active_text()
        } else {
            tokens::tab_text()
        })
        .when(!is_active, |tab| {
            tab.hover(|style| style.bg(tokens::tab_hover_bg()))
        })
        .on_click(
            cx.listener(move |this: &mut RibbonState, event: &ClickEvent, window: &mut Window, cx| {
                // Switching tabs dismisses any open dropdown.
                this.open_popup = None;
                let double_click = matches!(
                    event,
                    ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2
                );
                // Collapsing is a product capability, disabled by default;
                // double-click only toggles when the host enables it.
                if double_click && was_active && this.model.collapse_enabled {
                    this.emit(RibbonEvent::ToggleCollapse, window, cx);
                } else if !was_active {
                    this.emit(
                        RibbonEvent::ActiveTabChanged { id: tab_id.clone() },
                        window,
                        cx,
                    );
                }
            }),
        )
        .when(is_active, |tab| {
            tab.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(2.0))
                    .bg(tokens::tab_active_bar()),
            )
        })
        .when(tab.contextual, |tab| {
            // TODO(ribbon): contextual tabs switch to the amber accent
            // (tokens::contextual_accent) for the indicator bar and title.
            tab
        })
        .child(SharedString::from(tab.title.clone()))
        .into_any_element()
}

/// Shared ribbon button color variant: transparent at rest with hover/pressed
/// states; the checked form paints the accent 25% background plus accent
/// outline (`selected` renders `active` as the background and `color` as the
/// border).
pub(crate) fn button_variant(checked: bool, cx: &App) -> ButtonCustomVariant {
    if checked {
        ButtonCustomVariant::new(cx)
            .color(tokens::accent())
            .foreground(tokens::text())
            .hover(tokens::btn_checked_bg())
            .active(tokens::btn_checked_bg())
    } else {
        ButtonCustomVariant::new(cx)
            .color(tokens::btn_rest())
            .foreground(tokens::text())
            .hover(tokens::btn_hover())
            .active(tokens::btn_pressed())
    }
}

/// Tints an icon with the enabled/disabled text color.
pub(crate) fn icon_element(icon: gpui_component::Icon, enabled: bool) -> AnyElement {
    icon.text_color(icon_color(enabled)).into_any_element()
}

pub(crate) fn icon_color(enabled: bool) -> gpui::Hsla {
    if enabled {
        tokens::text()
    } else {
        tokens::text_disabled()
    }
}

/// 16px first-character placeholder block for Icon tier items without an icon.
pub(crate) fn placeholder_glyph(label: &str, color: gpui::Hsla) -> AnyElement {
    let first = label.chars().next().unwrap_or('?').to_string();
    div()
        .size(px(tokens::ICON_SMALL))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(2.0))
        .bg(tokens::btn_hover())
        .text_size(px(10.0))
        .text_color(color)
        .child(SharedString::from(first))
        .into_any_element()
}
