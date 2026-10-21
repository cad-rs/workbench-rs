//! UI theme (dark / light) for the gpui frontend shell.
//!
//! The shell's own colors live here (the ribbon crate carries its own token
//! set). [`apply_theme`] switches all three surfaces at once — the shell
//! palette, the ribbon tokens and the gpui-component global theme (dropdowns,
//! tooltips) — so the whole window stays consistent.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::App;
use workbench_api as wb;

/// Shell palette: the colors the gpui frontend paints outside the ribbon.
#[derive(Clone, Copy)]
pub struct UiTheme {
    /// Root window background.
    pub bg: wb::Color,
    /// Status bar background.
    pub toolbar: wb::Color,
    /// GPU viewport placeholder (failed / not-ready viewports).
    pub placeholder: wb::Color,
    /// Progress bar track on the status bar.
    pub progress_track: wb::Color,
    /// Cancel button resting background on the status bar.
    pub cancel_rest: wb::Color,
}

/// Dark product palette.
pub const DARK: UiTheme = UiTheme {
    bg: wb::Color::rgb(0.16, 0.17, 0.20),
    toolbar: wb::Color::rgb(0.20, 0.21, 0.25),
    placeholder: wb::Color::rgb(0.10, 0.11, 0.13),
    progress_track: wb::Color::rgb(0.12, 0.12, 0.15),
    cancel_rest: wb::Color::rgb(0.35, 0.22, 0.22),
};

/// Light palette.
pub const LIGHT: UiTheme = UiTheme {
    bg: wb::Color::rgb(0.94, 0.94, 0.94),
    toolbar: wb::Color::rgb(0.90, 0.90, 0.90),
    placeholder: wb::Color::rgb(0.85, 0.85, 0.86),
    progress_track: wb::Color::rgb(0.80, 0.80, 0.81),
    cancel_rest: wb::Color::rgb(0.90, 0.60, 0.60),
};

static ACTIVE_UI_THEME: AtomicU8 = AtomicU8::new(0);

/// Sets the shell palette from a theme name (`"dark"` / `"light"`, case
/// insensitive; anything else falls back to dark).
pub fn set_ui_theme(name: &str) {
    let light = name.eq_ignore_ascii_case("light");
    ACTIVE_UI_THEME.store(u8::from(light), Ordering::Relaxed);
    ribbon::set_theme(if light {
        ribbon::ThemeKind::Light
    } else {
        ribbon::ThemeKind::Dark
    });
}

/// The active shell palette.
pub fn ui_theme() -> &'static UiTheme {
    match ACTIVE_UI_THEME.load(Ordering::Relaxed) {
        1 => &LIGHT,
        _ => &DARK,
    }
}

/// Applies a named theme to every themed surface: the shell palette, the
/// ribbon tokens and the gpui-component global theme (dropdowns, tooltips).
/// Call after `gpui_component::init`.
pub fn apply_theme(name: &str, cx: &mut App) {
    set_ui_theme(name);
    let mode = if name.eq_ignore_ascii_case("light") {
        gpui_component::ThemeMode::Light
    } else {
        gpui_component::ThemeMode::Dark
    };
    gpui_component::Theme::change(mode, None, cx);
}

/// Parse `#RRGGBB` (or `RRGGBB`) into gpui's packed `0xRRGGBBAA` form;
/// None on malformed input.
pub fn parse_hex_rgb(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some((v << 8) | 0xFF)
}

/// Apply the `ui.accent` setting to the ribbon accent tokens (product brand).
/// Malformed input restores the default accent.
pub fn apply_accent(accent: &str) {
    ribbon::set_accent(parse_hex_rgb(accent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_rgb_with_and_without_hash() {
        assert_eq!(parse_hex_rgb("#3B82F6"), Some(0x3B82F6FF));
        assert_eq!(parse_hex_rgb("3B82F6"), Some(0x3B82F6FF));
        assert_eq!(parse_hex_rgb("#000000"), Some(0x000000FF));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(parse_hex_rgb("#3B82F"), None, "too short");
        assert_eq!(parse_hex_rgb("#3B82F6FF"), None, "too long");
        assert_eq!(parse_hex_rgb("#GGGGGG"), None, "non-hex");
        assert_eq!(parse_hex_rgb(""), None);
    }

    #[test]
    fn apply_accent_restores_default_on_garbage() {
        apply_accent("#FF0000");
        assert_eq!(ribbon::accent_override(), Some(0xFF0000FF));
        apply_accent("not-a-color");
        assert_eq!(ribbon::accent_override(), None, "garbage restores default");
    }
}
