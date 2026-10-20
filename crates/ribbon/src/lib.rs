//! A Fluent-style RibbonBar component for gpui, built on gpui-component.
//!
//! The ribbon renders a two-level chrome: a 30px tab strip (logo, quick access
//! toolbar, tabs, collapse toggle) and an 86px panel area with the active
//! tab's command groups (large / small / icon buttons, split and menu
//! dropdowns, combo boxes), following the classic Office Fluent layout.
//!
//! # Usage
//!
//! The host owns the data: it hands the component a [`RibbonModel`] snapshot
//! and receives [`RibbonEvent`]s back. The component understands no command
//! semantics of its own.
//!
//! ```ignore
//! let ribbon = cx.new(|_| {
//!     RibbonState::new(model, |this, event, window, cx| match event {
//!         RibbonEvent::Command { id, value } => { /* execute */ }
//!         RibbonEvent::ActiveTabChanged { id } => { /* set active tab */ }
//!         RibbonEvent::ToggleCollapse => { /* flip collapsed, set_model */ }
//!     })
//! });
//! // parent view render:
//! div().child(ribbon.clone())
//! ```
//!
//! Requires `gpui_component::init` (theme, key bindings for popup menus and
//! tooltips) to have been called at application startup.

mod layout;
mod model;
mod panel;
mod popup;
mod state;
mod tabs;
mod tokens;

pub use model::{
    ItemSize, MenuEntry, MenuEntryVm, OverflowStrategy, RibbonEvent, RibbonGroupVm, RibbonItemKindVm,
    RibbonItemVm, RibbonModel, RibbonTabVm,
};
pub use state::RibbonState;
pub use tokens::{set_theme, theme, ThemeKind};
