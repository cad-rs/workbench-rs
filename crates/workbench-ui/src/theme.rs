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
