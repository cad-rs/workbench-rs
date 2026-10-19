//! View model types for the RibbonBar.
//!
//! The component consumes a `RibbonModel` snapshot and emits `RibbonEvent`s.
//! It has no knowledge of any command system: every interaction is reported
//! back as an event and the host decides what to do with it.

/// Events emitted by the RibbonBar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RibbonEvent {
    /// A command was invoked by a button click, a menu entry selection or a
    /// combo selection. `value` carries the selected option for combos and is
    /// `None` for plain commands.
    Command { id: String, value: Option<String> },
    /// The user activated another tab.
    ActiveTabChanged { id: String },
    /// The user asked to toggle the collapsed (minimized) state of the ribbon.
    ToggleCollapse,
}

/// How the panel area reacts when its content is wider than the viewport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OverflowStrategy {
    /// Keep every command at a stable position and scroll the panel
    /// horizontally (default). Endpoint arrows appear while the content
    /// overflows.
    #[default]
    Scroll,
    /// Collapse the right-most groups into an overflow dropdown (Office
    /// Fluent style). Not implemented yet: rendered with the [`OverflowStrategy::Scroll`]
    /// fallback. TODO(ribbon): implement group collapse overflow.
    Collapse,
}

/// Visual size tier of an item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ItemSize {
    /// 24px icon on top of an 11px label, occupying a full 48px column.
    #[default]
    Large,
    /// 16px icon beside an 11.5px label on a 22px high row.
    Small,
    /// 16px icon only, no label, on a 22px high row.
    Icon,
}

/// Behavioral kind of a ribbon item.
#[derive(Debug, Clone, PartialEq)]
pub enum RibbonItemKindVm {
    /// Plain command button.
    Command,
    /// Two-state button; `checked` comes from the host state.
    Toggle { checked: bool },
    /// Main area runs the default command, the arrow opens `items`.
    Split { items: Vec<MenuEntry> },
    /// The whole button opens `items` as a dropdown menu.
    Menu { items: Vec<MenuEntry> },
    /// Select-only combo box; `current` is shown in the value area, selecting
    /// an option emits `RibbonEvent::Command` with `value: Some(option)`.
    Combo { current: String, options: Vec<String> },
}

/// One selectable entry inside a dropdown menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntryVm {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub enabled: bool,
    pub checked: bool,
}

/// A dropdown entry or a separator.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuEntry {
    Item(MenuEntryVm),
    Separator,
}

/// A single ribbon command item.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonItemVm {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub enabled: bool,
    pub size: ItemSize,
    /// Preferred width in logical pixels. Only used by [`RibbonItemKindVm::Combo`].
    pub width: Option<f32>,
    pub kind: RibbonItemKindVm,
}

/// A titled group of items, drawn with a centered caption and separated from
/// neighboring groups by a 1px vertical line.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonGroupVm {
    pub id: String,
    pub title: String,
    pub items: Vec<RibbonItemVm>,
}

/// One tab of the ribbon with its groups.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonTabVm {
    pub id: String,
    pub title: String,
    /// Contextual tabs are highlighted differently once contextual styling
    /// lands. TODO(ribbon): use the amber contextual accent.
    pub contextual: bool,
    pub groups: Vec<RibbonGroupVm>,
}

/// The full snapshot consumed by the RibbonBar each frame.
///
/// Contextual visibility is resolved by the host (bridge): tabs listed here are
/// rendered as-is; the component does not filter by `work_mode`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RibbonModel {
    /// Logo icon resource path (svg), drawn at the left edge of the tab strip.
    pub logo: Option<String>,
    /// Quick access toolbar items, always rendered as Icon tier buttons on the
    /// tab strip (also visible while collapsed).
    pub qat: Vec<RibbonItemVm>,
    pub tabs: Vec<RibbonTabVm>,
    pub active_tab_id: String,
    /// When true the panel area is hidden and only the 30px tab strip remains.
    pub collapsed: bool,
    pub overflow: OverflowStrategy,
    /// Active work mode name. Kept for diagnostics; filtering already happened
    /// in the bridge.
    pub work_mode: String,
}
