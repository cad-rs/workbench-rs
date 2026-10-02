//! workbench-rs 的 Python 插件宿主（design.md §11）。
//!
//! 提供 `plugin_stage(dirs)`：插件发现、清单与 API 版本检查、加载与初始化、
//! 命令/工具栏贡献，以及错误隔离（失败插件被拒绝并记录原因，应用继续运行）。
//! 进程内 Python 不构成安全沙箱——权限声明仅用于审计与提示（§11.2/§11.5）。

pub mod host;
pub mod loader;
pub mod manifest;

pub use host::{Collected, HostApi, PyTask, PluginCancelled};
pub use loader::{discover, plugin_stage};
pub use manifest::{parse as parse_manifest, split_entry_point, PluginManifest, HOST_API_VERSION};
