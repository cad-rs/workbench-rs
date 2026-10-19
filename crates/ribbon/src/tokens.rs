//! Named color and metric tokens for the RibbonBar (dark theme).
//!
//! All colors are centralized here so the component never carries bare color
//! values. Values follow the product dark palette; a light theme would add a
//! second set of accessors later without touching call sites.

use gpui::{ColorExt as _, Hsla, hsla, rgb, rgb_to_hsla};

// -- Colors -----------------------------------------------------------------

/// Ribbon body background (`COL_TOOLBAR`).
pub fn ribbon_bg() -> Hsla {
    rgb_to_hsla(rgb(0x333640))
}

/// Tab strip background, one step darker than the panel (`COL_BG`).
pub fn tab_strip_bg() -> Hsla {
    rgb_to_hsla(rgb(0x292B33))
}

/// Non-active tab text (`LIGHT_GRAY`).
pub fn tab_text() -> Hsla {
    rgb_to_hsla(rgb(0xC7C7CC))
}

/// Active tab text (white).
pub fn tab_active_text() -> Hsla {
    rgb_to_hsla(rgb(0xFFFFFF))
}

/// Active tab 2px indicator bar (accent).
pub fn tab_active_bar() -> Hsla {
    accent()
}

/// Tab hover background.
pub fn tab_hover_bg() -> Hsla {
    rgb_to_hsla(rgb(0x3D404D))
}

/// Button resting background (transparent, melts into `ribbon_bg`).
pub fn btn_rest() -> Hsla {
    transparent()
}

/// Button hover background.
pub fn btn_hover() -> Hsla {
    rgb_to_hsla(rgb(0x4D5261))
}

/// Button pressed background.
pub fn btn_pressed() -> Hsla {
    rgb_to_hsla(rgb(0x424754))
}

/// Checked toggle background (accent at 25% alpha).
pub fn btn_checked_bg() -> Hsla {
    accent().opacity(0.25)
}

/// Accent color used for the active tab bar, checked toggles and focus.
pub fn accent() -> Hsla {
    rgb_to_hsla(rgb(0x338CF2))
}

// Amber accent for contextual tab indicators is introduced together with
// contextual tab styling (rgb(0.9, 0.7, 0.2)); the first version keeps the
// regular accent.

/// Usable button label / icon color (white).
pub fn text() -> Hsla {
    rgb_to_hsla(rgb(0xFFFFFF))
}

/// Disabled label / icon color (`GRAY`).
pub fn text_disabled() -> Hsla {
    rgb_to_hsla(rgb(0x80808C))
}

/// Secondary text (group captions).
pub fn text_secondary() -> Hsla {
    text_disabled()
}

/// Ribbon bottom 1px border.
pub fn ribbon_border() -> Hsla {
    rgb_to_hsla(rgb(0x1A1A1F))
}

/// Group vertical separator line and combo box outline.
pub fn group_sep() -> Hsla {
    rgb_to_hsla(rgb(0x4D4F5C))
}

/// Fully transparent color.
pub fn transparent() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.0)
}

// -- Metrics (logical pixels) -----------------------------------------------

/// Tab strip height.
pub const TAB_STRIP_HEIGHT: f32 = 30.0;
/// Panel area height (large item 86 = icon 48 + label 26 + chevron reserve
/// 12, + group title 16 + margins 8).
pub const PANEL_HEIGHT: f32 = 110.0;
/// Large item icon zone height.
pub const BUTTON_ZONE_HEIGHT: f32 = 48.0;
/// Two-line label strip (2 x 13px lines) below the icon zone of a large
/// item; a third line is clipped.
pub const LABEL_HEIGHT: f32 = 26.0;
/// Line height of one large-item label line.
pub const LABEL_LINE_HEIGHT: f32 = 13.0;
/// Chevron strip below the label of a Large Split/Menu button (6 o'clock
/// position: below the text, centered, never covering or moving it).
pub const CHEVRON_STRIP_HEIGHT: f32 = 12.0;
/// Group caption strip height.
pub const GROUP_TITLE_HEIGHT: f32 = 16.0;
/// Row height of Small / Icon / Combo items.
/// Small-item row height: one third of the large-item column (86/3). Small
/// columns are a fixed 3-slot grid — columns with fewer items leave the
/// lower slots empty; there is no 2-way division.
pub const ROW_HEIGHT: f32 = 28.67;
/// Column width occupied by a Large item.
pub const LARGE_COLUMN_WIDTH: f32 = 48.0;
/// Default combo box width (min 60 / max 300, declarable override).
pub const COMBO_DEFAULT_WIDTH: f32 = 120.0;
pub const COMBO_MIN_WIDTH: f32 = 60.0;
pub const COMBO_MAX_WIDTH: f32 = 300.0;
/// Maximum rows in one stacked column of Small/Icon/Combo items.
pub const MAX_STACK_ROWS: usize = 3;
/// Small icon side.
pub const ICON_SMALL: f32 = 16.0;
/// Large icon side.
pub const ICON_LARGE: f32 = 24.0;
/// Logo side.
pub const LOGO_SIZE: f32 = 20.0;
/// QAT / collapse hit area side.
pub const QAT_SIZE: f32 = 22.0;
/// Vertical gap between a trigger button and its dropdown.
pub const POPUP_GAP: f32 = 4.0;
/// Minimum dropdown width.
pub const POPUP_MIN_WIDTH: f32 = 160.0;
/// Dropdown margin kept to the window content edges.
pub const POPUP_EDGE_MARGIN: f32 = 8.0;
/// Width of the overlay scroll arrows.
pub const SCROLL_ARROW_WIDTH: f32 = 15.0;

/// Combo width resolution: declared width clamped to [60, 300], else default.
pub fn combo_width(declared: Option<f32>) -> f32 {
    declared
        .unwrap_or(COMBO_DEFAULT_WIDTH)
        .clamp(COMBO_MIN_WIDTH, COMBO_MAX_WIDTH)
}
