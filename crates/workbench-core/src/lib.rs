//! workbench-rs 平台运行时。
//!
//! 下游整合入口：
//! - [`WorkbenchAppBuilder`]：产品装配（选择 Workbench 模块、插件阶段、默认布局）；
//! - [`cli`]：产品命令行约定（运行 / `--selfcheck` / `--selfcheck-crash` / `--smoke`）；
//! - [`selfcheck`]：无头自检（探测式，无关项自动跳过）。

pub mod app;
pub mod cli;
pub mod dialog;
pub mod executor;
pub mod platform_cmds;
pub mod platform_panels;
pub mod selfcheck;

pub use app::{AppRuntime, PluginStage, WorkbenchAppBuilder};
pub use executor::AsyncExecutor;
