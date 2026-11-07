//! Rhai script plugin host: manifest discovery, Host API v1 (command / toolbar / script panel
//! registration).
//!
//! Decoupled from the platform core: this crate depends only on workbench-api/core and hooks
//! into product assembly through [`plugin_stage`] (the same isolation policy as any other
//! PluginStage). Script UI panels are mounted and rendered by the gpui frontend via gpui-rhai
//! (`PanelDef.script`).

mod host;
mod loader;
mod manifest;

pub use host::{args_to_map, empty_map, Collected, CollectedCommand, RhaiProgram, SharedCollected, CANCEL_SENTINEL};
pub use loader::{discover, plugin_stage};
pub use manifest::{parse as parse_manifest, split_entry_point, PluginManifest, HOST_API_VERSION};
