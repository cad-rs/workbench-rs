//! The RibbonBar entity: model snapshot, transient state and top-level render.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, Context, Focusable as _, IntoElement, Pixels, ParentElement as _, Render,
    ScrollWheelEvent, Styled as _, Window, px,
};

use crate::model::{RibbonEvent, RibbonModel, RibbonTabVm};
use crate::popup::{DropdownRequest, PopupHandle, TriggerBoundsCell, new_trigger_cell};
use crate::{panel, popup, tabs, tokens};

type OnEvent = Box<dyn Fn(&mut RibbonState, RibbonEvent, &mut Window, &mut App) + 'static>;

/// The RibbonBar component state.
///
/// The host owns the [`RibbonModel`] (data) and receives [`RibbonEvent`]s
/// (behavior); this state only carries internal transients (panel scroll
/// offset, the open dropdown) which survive [`RibbonState::set_model`].
pub struct RibbonState {
    /// Current model snapshot; the bridge replaces it via `set_model`.
    pub(crate) model: RibbonModel,
    on_event: OnEvent,
    /// Panel horizontal scroll offset in logical pixels (transient).
    scroll_offset: f32,
    /// Active scroll animation state, if a page-scroll is in progress (transient).
    scroll_animation: Option<ScrollAnimation>,
    /// The single open dropdown, if any (transient).
    pub(crate) open_popup: Option<PopupHandle>,
    /// Viewport width of the panel scroll area, measured during prepaint.
    pub(crate) panel_viewport_w: Rc<Cell<f32>>,
    /// Total content width of the panel scroll area, measured during prepaint.
    pub(crate) panel_content_w: Rc<Cell<f32>>,
}

/// In-progress page scroll animation: eases from `start_offset` to
/// `target_offset` over 150 ms with an ease-out cubic curve.
struct ScrollAnimation {
    start_offset: f32,
    target_offset: f32,
    start_time: std::time::Instant,
}

impl RibbonState {
    pub fn new(
        model: RibbonModel,
        on_event: impl Fn(&mut Self, RibbonEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            model,
            on_event: Box::new(on_event),
            scroll_offset: 0.0,
            scroll_animation: None,
            open_popup: None,
            panel_viewport_w: Rc::new(Cell::new(0.0)),
            panel_content_w: Rc::new(Cell::new(0.0)),
        }
    }

    /// Replaces the model snapshot. Called by the bridge every frame.
    ///
    /// Transients (scroll offset, open dropdown) are preserved; the dropdown is
    /// force-closed when the collapse state or the active tab changed, per the
    /// popup dismissal rules.
    pub fn set_model(&mut self, model: RibbonModel, cx: &mut Context<Self>) {
        let close_popup = model.collapsed != self.model.collapsed
            || model.active_tab_id != self.model.active_tab_id;
        self.model = model;
        if close_popup {
            self.open_popup = None;
        }
        cx.notify();
    }

    /// Read access to the current model snapshot.
    pub fn model(&self) -> &RibbonModel {
        &self.model
    }

    /// Dispatches an event to the host callback and repaints.
    pub(crate) fn emit(&mut self, event: RibbonEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Swap the callback out so it can receive `&mut Self` while running.
        let callback = std::mem::replace(&mut self.on_event, Box::new(|_, _, _, _| {}));
        callback(self, event, window, cx);
        self.on_event = callback;
        cx.notify();
    }

    /// Opens a dropdown anchored to `trigger` (window-space bounds).
    pub(crate) fn open_dropdown(
        &mut self,
        trigger: Bounds<Pixels>,
        request: DropdownRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Drop any open popup first: focusing the new menu dismisses the old
        // one, and the stale DismissEvent callback would then steal the fresh
        // handle back out of `open_popup` (reopen race).
        self.open_popup = None;
        let menu = popup::build_menu(&request, window, cx);
        let restore_focus = window.focused(cx);
        // Focus the menu so Esc and the arrow keys work while it is open.
        menu.focus_handle(cx).focus(window, cx);

        let this = cx.entity().downgrade();
        let subscription =
            window.subscribe(&menu, cx, move |_, _: &gpui::DismissEvent, window, cx| {
                _ = this.update(cx, |this, cx| {
                    if let Some(handle) = this.open_popup.take()
                        && let Some(focus) = handle.restore_focus.clone()
                    {
                        focus.focus(window, cx);
                    }
                    cx.notify();
                });
            });

        self.open_popup = Some(PopupHandle::new(menu, trigger, restore_focus, subscription));
        cx.notify();
    }

    /// A fresh measurement cell for a dropdown trigger.
    pub(crate) fn new_popup_trigger_cell(&self) -> TriggerBoundsCell {
        new_trigger_cell()
    }

    /// The tab to display: the declared active tab, falling back to the first
    /// tab when the id is unknown (unregistered tab recovery rule).
    pub(crate) fn active_tab(&self) -> Option<&RibbonTabVm> {
        let tabs = &self.model.tabs;
        tabs.iter()
            .find(|tab| tab.id == self.model.active_tab_id)
            .or_else(|| tabs.first())
    }

    /// Clamped scroll offset for the currently measured widths.
    pub(crate) fn clamped_scroll_offset(&self) -> f32 {
        let max_offset = (self.panel_content_w.get() - self.panel_viewport_w.get()).max(0.0);
        self.scroll_offset.clamp(0.0, max_offset)
    }

    /// Viewport width of the panel scroll area (measured last frame).
    pub(crate) fn viewport_width(&self) -> f32 {
        self.panel_viewport_w.get()
    }

    /// Total content width of the panel scroll area (measured last frame).
    pub(crate) fn content_width(&self) -> f32 {
        self.panel_content_w.get()
    }

    pub(crate) fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(window.line_height());
        let mut horizontal = delta.x;
        // Shift + vertical wheel scrolls horizontally; a bare vertical wheel
        // never scrolls the ribbon (the wheel stays reserved for the viewport
        // semantics of the CAD application).
        if event.modifiers.shift {
            horizontal += delta.y;
        }
        if horizontal == px(0.0) {
            return;
        }
        // Any scrolling dismisses the open dropdown and cancels any
        // in-progress page-scroll animation.
        self.open_popup = None;
        self.scroll_animation = None;
        let max_offset = (self.panel_content_w.get() - self.panel_viewport_w.get()).max(0.0);
        self.scroll_offset = (self.scroll_offset + f32::from(horizontal)).clamp(0.0, max_offset);
        cx.stop_propagation();
        cx.notify();
    }

    /// Advances the page-scroll animation by one frame: interpolates
    /// `scroll_offset` along an ease-out cubic curve over 150 ms, then
    /// re-registers itself via `on_next_frame` until the duration elapses.
    fn tick_scroll_animation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(anim) = self.scroll_animation.as_ref() else {
            return;
        };
        let elapsed_ms = anim.start_time.elapsed().as_millis() as f32;
        let t = (elapsed_ms / SCROLL_ANIM_DURATION_MS).min(1.0);
        let eased = ease_out_cubic(t);
        self.scroll_offset =
            anim.start_offset + (anim.target_offset - anim.start_offset) * eased;
        if t < 1.0 {
            cx.on_next_frame(window, move |this, window, cx| {
                this.tick_scroll_animation(window, cx);
            });
        } else {
            // Snap to the exact target and clear the animation state.
            self.scroll_offset = anim.target_offset;
            self.scroll_animation = None;
        }
        cx.notify();
    }

    /// Scrolls the panel by roughly 80% of the viewport (endpoint arrows)
    /// with a 150 ms ease-out animation.
    pub(crate) fn scroll_by_page(&mut self, direction: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.open_popup = None;
        let viewport = self.panel_viewport_w.get().max(0.0);
        let max_offset = (self.panel_content_w.get() - viewport).max(0.0);
        let start = self.clamped_scroll_offset();
        let target = (start + direction * viewport * 0.8).clamp(0.0, max_offset);
        // No-op if already at the target.
        if (target - start).abs() < 0.5 {
            return;
        }
        self.scroll_animation = Some(ScrollAnimation {
            start_offset: start,
            target_offset: target,
            start_time: std::time::Instant::now(),
        });
        cx.on_next_frame(window, move |this, window, cx| {
            this.tick_scroll_animation(window, cx);
        });
    }
}

impl Render for RibbonState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // While collapsed only the 30px tab strip remains (QAT stays usable).
        let collapsed = self.model.collapsed;

        let root = gpui::div()
            .flex()
            .flex_col()
            .w_full()
            .bg(tokens::ribbon_bg())
            .border_b_1()
            .border_color(tokens::ribbon_border())
            .overflow_hidden()
            .child(tabs::render_tab_strip(self, cx))
            .when(!collapsed, |root| root.child(panel::render_panel(self, cx)));

        match self.open_popup.as_ref() {
            Some(handle) => root.child(popup::render_popup(handle, window)),
            None => root,
        }
    }
}

/// Duration of the page-scroll animation in milliseconds.
const SCROLL_ANIM_DURATION_MS: f32 = 150.0;

/// Ease-out cubic: fast start, gentle deceleration to the target.
fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}
