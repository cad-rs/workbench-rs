//! workbench-rs platform runtime.
//!
//! Integration entry points for downstream code:
//! - [`WorkbenchAppBuilder`]: product assembly (choose Workbench modules,
//!   plugin stages, default layout);
//! - [`cli`]: product CLI conventions (run / `--selfcheck` /
//!   `--selfcheck-crash` / `--smoke`);
//! - [`selfcheck`]: headless selfcheck (probe-based; unrelated items SKIP).

pub mod app;
pub mod cli;
pub mod dialog;
pub mod executor;
pub mod platform_cmds;
pub mod platform_panels;
pub mod selfcheck;

pub use app::{AppRuntime, PluginStage, WorkbenchAppBuilder};
pub use executor::AsyncExecutor;
