//! A GUI-independent asset loading contract (SVG icons and other static assets).
//!
//! Workbenches/plugins declare asset paths on UI elements (such as `"icons/save.svg"`), the
//! product supplies an [`AssetSource`] through [`crate::WorkbenchAppBuilder::with_assets`]
//! (workbench-core), and the frontend fetches the bytes by path and renders them. A path is only
//! a stable key and need not correspond to a real file — the most common form is a static table
//! embedded in the binary with `include_bytes!`.

use std::borrow::Cow;
use std::sync::Arc;

/// A static asset source: loads asset bytes by path.
///
/// Returning `None` means this source does not have the asset (the caller keeps looking further
/// down or falls back to showing no icon); a load failure should also return `None` rather than
/// break the UI.
pub trait AssetSource: Send + Sync {
    fn load(&self, path: &str) -> Option<Cow<'static, [u8]>>;

    /// Enumerate every asset path this source holds (used by the GUI frontend to register paths
    /// with gpui's `AssetRegistry`; the registry only resolves registered paths). Empty by
    /// default — a purely on-demand source cannot enumerate itself and should switch to an
    /// enumerable form (such as [`StaticAssets`]).
    fn paths(&self) -> Vec<String> {
        Vec::new()
    }
}

/// A chained asset source: looks up in order, and the first hit wins (later sources are
/// fallbacks).
///
/// Typical use: `ChainedAssets::new(vec![product_source, platform_standard_source])` — the
/// product can override the platform's standard icons with the same paths, or just add icons of
/// its own.
pub struct ChainedAssets {
    sources: Vec<Arc<dyn AssetSource>>,
}

impl ChainedAssets {
    pub fn new(sources: Vec<Arc<dyn AssetSource>>) -> Self {
        Self { sources }
    }
}

impl AssetSource for ChainedAssets {
    fn load(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        self.sources.iter().find_map(|s| s.load(path))
    }

    fn paths(&self) -> Vec<String> {
        self.sources.iter().flat_map(|s| s.paths()).collect()
    }
}

/// Convenience constructor: build an asset source from a static `(path, bytes)` table (use with
/// `include_bytes!`).
#[derive(Clone, Copy)]
pub struct StaticAssets {
    entries: &'static [(&'static str, &'static [u8])],
}

impl StaticAssets {
    pub const fn new(entries: &'static [(&'static str, &'static [u8])]) -> Self {
        Self { entries }
    }
}

impl AssetSource for StaticAssets {
    fn load(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        self.entries
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes))
    }

    fn paths(&self) -> Vec<String> {
        self.entries.iter().map(|(p, _)| (*p).to_string()).collect()
    }
}
