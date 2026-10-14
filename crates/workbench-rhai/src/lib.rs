//! Rhai 脚本插件宿主：清单发现、Host API v1（命令/工具栏/脚本面板注册）。
//!
//! 与平台核心解耦：本 crate 只依赖 workbench-api/core，通过
//! [`plugin_stage`] 挂进产品装配（与其它 PluginStage 相同的隔离策略）。
//! 脚本 UI 面板由 gpui 前端经 gpui-rhai 挂载渲染（`PanelDef.script`）。

mod host;
mod loader;
mod manifest;

pub use host::{args_to_map, empty_map, Collected, CollectedCommand, RhaiProgram, SharedCollected, CANCEL_SENTINEL};
pub use loader::{discover, plugin_stage};
pub use manifest::{parse as parse_manifest, split_entry_point, PluginManifest, HOST_API_VERSION};
