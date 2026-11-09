//! workbench-rs platform runtime.
//!
//! Integration entry points for downstream code:
//! - [`WorkbenchAppBuilder`]: product assembly (choose Workbench modules,
//!   plugin stages, default layout);
//! - [`cli`]: product CLI conventions (run / `--selfcheck` /
//!   `--selfcheck-crash` / `--smoke`);
//! - [`diagnostics`]: log file and crash reports (call `init` from `main`);
//! - [`selfcheck`]: headless selfcheck (probe-based; unrelated items SKIP).

pub mod account;
pub mod app;
pub mod assets;
pub mod cli;
pub mod diagnostics;
mod entitlement_refresh;
#[cfg(not(target_arch = "wasm32"))]
mod atomic;
#[cfg(not(target_arch = "wasm32"))]
mod dialog;
pub mod executor;
pub mod file_picker;
#[cfg(not(target_arch = "wasm32"))]
mod local_files;
#[cfg(target_arch = "wasm32")]
mod js;
#[cfg(target_arch = "wasm32")]
mod opfs;
pub mod platform_cmds;
pub mod product;
pub mod selfcheck;
mod state;
#[cfg(target_arch = "wasm32")]
mod web_files;

pub use app::{AppRuntime, PluginStage, WorkbenchAppBuilder};
pub use executor::AsyncExecutor;
pub use product::{discover as discover_product, load_from_dirs as load_product_from};
