//! Platform standard assets: SVG icons for the platform's built-in commands (embedded with
//! `include_bytes!`).
//!
//! Icon paths are prefixed with `wb-std/` and served by [`standard_assets`]. The product's
//! asset source comes before the standard one in the [`workbench_api::ChainedAssets`] chain,
//! so it can override an icon by using the same path.
//!
//! Semantics: icons render as alpha masks tinted by the frontend (monochrome icons); the SVG's
//! own colors have no effect, the stroke/fill shapes define the mask.

use workbench_api::StaticAssets;

/// Platform standard icon paths.
pub mod icons {
    pub const NEW: &str = "wb-std/new.svg";
    pub const OPEN: &str = "wb-std/open.svg";
    pub const SAVE: &str = "wb-std/save.svg";
    pub const UNDO: &str = "wb-std/undo.svg";
    pub const REDO: &str = "wb-std/redo.svg";
}

static STANDARD_ENTRIES: &[(&str, &[u8])] = &[
    (icons::NEW, include_bytes!("../assets/std/new.svg")),
    (icons::OPEN, include_bytes!("../assets/std/open.svg")),
    (icons::SAVE, include_bytes!("../assets/std/save.svg")),
    (icons::UNDO, include_bytes!("../assets/std/undo.svg")),
    (icons::REDO, include_bytes!("../assets/std/redo.svg")),
];

/// The platform standard icon asset source (global static, allocation-free lookup).
pub fn standard_assets() -> &'static StaticAssets {
    static ASSETS: StaticAssets = StaticAssets::new(STANDARD_ENTRIES);
    &ASSETS
}
