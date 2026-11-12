//! Rhai script plugin host for workbench-rs: manifest discovery, Host API v1
//! (command / toolbar / script panel registration).
//!
//! The manifest, the Rhai engines and the loading pipeline are the shared
//! `plugin-host` crate (product-kit); this crate is the workbench adapter that
//! commits a plugin's declarations into the [`workbench_api::Registry`].
//!
//! Decoupled from the platform core: hooks into product assembly through
//! [`plugin_stage`] (the same isolation policy as any other PluginStage).
//! Script UI panels are mounted and rendered by the gpui frontend via
//! gpui-rhai (`PanelDef.script`).

mod loader;

pub use loader::{discover, plugin_stage};
pub use plugin_host::{split_entry_point, PluginManifest, HOST_API_VERSION};

/// Parse and validate a manifest; on failure returns a readable error (the
/// loader records it as the rejection reason).
pub fn parse_manifest(text: &str) -> Result<PluginManifest, String> {
    let manifest = plugin_host::parse(text)?;
    plugin_host::validate(&manifest)?;
    Ok(manifest)
}
