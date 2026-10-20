//! Named color and metric tokens for the RibbonBar (dark + light themes).
//!
//! All colors are centralized here so the component never carries bare color
//! values. The active theme is a process-wide setting (`set_theme`); every
//! color accessor resolves through it, so call sites stay theme-free. A
//! metric never depends on the theme.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{ColorExt as _, Hsla, hsla, rgb, rgb_to_hsla};

// -- Theme selection --------------------------------------------------------

/// The available ribbon themes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeKind {
    /// Dark product palette (default).
    #[default]
    Dark = 0,
    /// Light palette (Office-like).
    Light = 1,
}

impl ThemeKind {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Light,
            _ => Self::Dark,
        }
    }
}

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(0);

/// Sets the process-wide ribbon theme.
pub fn set_theme(kind: ThemeKind) {
    ACTIVE_THEME.store(kind as u8, Ordering::Relaxed);
}

/// The active ribbon theme.
pub fn theme() -> ThemeKind {
    ThemeKind::from_u8(ACTIVE_THEME.load(Ordering::Relaxed))
}

fn pick(dark: Hsla, light: Hsla) -> Hsla {
    match theme() {
        ThemeKind::Dark => dark,
        ThemeKind::Light => light,
    }
}

// -- Colors -----------------------------------------------------------------

/// Ribbon body background (`COL_TOOLBAR`).
pub fn ribbon_bg() -> Hsla {
    pick(rgb_to_hsla(rgb(0x333640)), rgb_to_hsla(rgb(0xF3F2F1)))
}

/// Tab strip background, one step darker than the panel (`COL_BG`).
pub fn tab_strip_bg() -> Hsla {
    pick(rgb_to_hsla(rgb(0x292B33)), rgb_to_hsla(rgb(0xE8E6E3)))
}

/// Non-active tab text (`LIGHT_GRAY`).
pub fn tab_text() -> Hsla {
    pick(rgb_to_hsla(rgb(0xC7C7CC)), rgb_to_hsla(rgb(0x4A4A4A)))
}

/// Active tab text (white / near-black).
pub fn tab_active_text() -> Hsla {
    pick(rgb_to_hsla(rgb(0xFFFFFF)), rgb_to_hsla(rgb(0x1A1A1A)))
}

/// Active tab 2px indicator bar (accent).
pub fn tab_active_bar() -> Hsla {
    accent()
}

/// Tab hover background.
pub fn tab_hover_bg() -> Hsla {
    pick(rgb_to_hsla(rgb(0x3D404D)), rgb_to_hsla(rgb(0xD9D7D4)))
}

/// Button resting background (transparent, melts into `ribbon_bg`).
pub fn btn_rest() -> Hsla {
    transparent()
}

/// Button hover background.
pub fn btn_hover() -> Hsla {
    pick(rgb_to_hsla(rgb(0x4D5261)), rgb_to_hsla(rgb(0xE0DFDC)))
}

/// Button pressed background.
pub fn btn_pressed() -> Hsla {
    pick(rgb_to_hsla(rgb(0x424754)), rgb_to_hsla(rgb(0xD4D2CF)))
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

/// Usable button label / icon color (white on dark, near-black on light).
pub fn text() -> Hsla {
    pick(rgb_to_hsla(rgb(0xFFFFFF)), rgb_to_hsla(rgb(0x242424)))
}

/// Disabled label / icon color.
pub fn text_disabled() -> Hsla {
    pick(rgb_to_hsla(rgb(0x80808C)), rgb_to_hsla(rgb(0x9A9A9A)))
}

/// Secondary text (group captions).
pub fn text_secondary() -> Hsla {
    text_disabled()
}

/// Ribbon bottom 1px border.
pub fn ribbon_border() -> Hsla {
    pick(rgb_to_hsla(rgb(0x1A1A1F)), rgb_to_hsla(rgb(0xD0CECD)))
}

/// Group vertical separator line and combo box outline.
pub fn group_sep() -> Hsla {
    pick(rgb_to_hsla(rgb(0x4D4F5C)), rgb_to_hsla(rgb(0xC8C6C4)))
}

/// Popup surface background (menu dropdowns).
pub fn popup_bg() -> Hsla {
    pick(rgb_to_hsla(rgb(0x2A2C34)), rgb_to_hsla(rgb(0xFFFFFF)))
}

/// Popup hover row background.
pub fn popup_hover_bg() -> Hsla {
    pick(rgb_to_hsla(rgb(0x3D404D)), rgb_to_hsla(rgb(0xE8E6E3)))
}

/// Fully transparent color.
pub fn transparent() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.0)
}

// -- Metrics (logical pixels) -----------------------------------------------

/// Tab strip height.
pub const TAB_STRIP_HEIGHT: f32 = 30.0;
/// Panel area height (large item 80 = icon 48 + label 26 + chevron reserve
/// 6, + group title 16 + margins 8).
pub const PANEL_HEIGHT: f32 = 104.0;
/// Large item icon zone height.
pub const BUTTON_ZONE_HEIGHT: f32 = 48.0;
/// Two-line label strip (2 x 13px lines) below the icon zone of a large
/// item; a third line is clipped.
pub const LABEL_HEIGHT: f32 = 26.0;
/// Line height of one large-item label line.
pub const LABEL_LINE_HEIGHT: f32 = 13.0;
/// Chevron strip below the label of a Large Split/Menu button (6 o'clock
/// position: below the text, centered, never covering or moving it). Every
/// Large button reserves this row so icons and labels sit at identical y
/// positions whether or not a chevron is present.
pub const CHEVRON_STRIP_HEIGHT: f32 = 6.0;
/// Group caption strip height.
pub const GROUP_TITLE_HEIGHT: f32 = 16.0;
/// Small-item row height: one third of the large-item column (80/3). Small
/// columns are a fixed 3-slot grid — columns with fewer items leave the
/// lower slots empty; there is no 2-way division.
pub const ROW_HEIGHT: f32 = 26.67;
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
/// Icon-tier button width in an icon row (16px icon + 2x4px padding).
pub const ICON_BUTTON_WIDTH: f32 = 24.0;
/// Extra width for an icon-tier button carrying a chevron.
pub const ICON_CHEVRON_EXTRA: f32 = 12.0;
/// Maximum icon-tier buttons on one icon row.
pub const ICONS_PER_ROW: usize = 4;
/// Viewport slack (logical px) required before a degraded group upgrades
/// back — the hysteresis band that keeps the degrade oscillation-free.
pub const DEGRADE_HYSTERESIS: f32 = 16.0;

/// Combo width resolution: declared width clamped to [60, 300], else default.
pub fn combo_width(declared: Option<f32>) -> f32 {
    declared
        .unwrap_or(COMBO_DEFAULT_WIDTH)
        .clamp(COMBO_MIN_WIDTH, COMBO_MAX_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_roundtrips_and_palettes_differ() {
        let dark_bg = ribbon_bg();
        let dark_text = text();
        set_theme(ThemeKind::Light);
        let light_bg = ribbon_bg();
        let light_text = text();
        assert_ne!(dark_bg, light_bg, "background must differ per theme");
        assert_ne!(dark_text, light_text, "text must differ per theme");
        assert_eq!(theme(), ThemeKind::Light);
        set_theme(ThemeKind::Dark);
        assert_eq!(theme(), ThemeKind::Dark);
        assert_eq!(ribbon_bg(), dark_bg);
        // Metrics are theme-independent.
        assert_eq!(PANEL_HEIGHT, 104.0);
    }
}
