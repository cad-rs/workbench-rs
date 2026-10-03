//! Python plugin host for workbench-rs (design.md §11).
//!
//! Provides `plugin_stage(dirs)`: plugin discovery, manifest and API version
//! checks, loading and initialization, command/toolbar contributions, and
//! error isolation (a failing plugin is rejected with its reason while the
//! application keeps running). In-process Python is not a security sandbox —
//! permission declarations are for auditing and prompting only (§11.2/§11.5).

pub mod host;
pub mod loader;
pub mod manifest;

pub use host::{Collected, HostApi, PyTask, PluginCancelled};
pub use loader::{discover, plugin_stage};
pub use manifest::{parse as parse_manifest, split_entry_point, PluginManifest, HOST_API_VERSION};
