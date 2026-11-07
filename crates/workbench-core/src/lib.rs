//! workbench-rs platform runtime.
//!
//! Integration entry points for downstream code:
//! - [`WorkbenchAppBuilder`]: product assembly (choose Workbench modules,
//!   plugin stages, default layout);
//! - [`cli`]: product CLI conventions (run / `--selfcheck` /
//!   `--selfcheck-crash` / `--smoke`);
//! - [`selfcheck`]: headless selfcheck (probe-based; unrelated items SKIP).

pub mod account;
pub mod app;
pub mod assets;
pub mod cli;
pub mod dialog;
pub mod executor;
#[cfg(not(target_arch = "wasm32"))]
mod local_files;
pub mod platform_cmds;
pub mod product;
pub mod selfcheck;

pub use app::{AppRuntime, PluginStage, WorkbenchAppBuilder};
pub use executor::AsyncExecutor;
pub use product::{discover as discover_product, load_from_dirs as load_product_from};
