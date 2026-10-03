//! Product command-line entry helpers (keeps downstream product `main`s tiny).
//!
//! Parses the common modes:
//! - (default) run the GUI;
//! - `--selfcheck`: headless selfcheck, process exit code 0/1;
//! - `--selfcheck-crash`: crash-recovery probe (invoked by the selfcheck as a
//!   subprocess; requires the `WB_CRASH_DIR` environment variable);
//! - `--smoke [secs]`: run for the given seconds and exit (default 2.5),
//!   for automated smoke tests.
//!
//! A typical product `main` (full example in the README "Integration guide"):
//!
//! ```text
//! fn main() {
//!     let cli = workbench_core::cli::from_env();
//!     let builder = workbench_core::WorkbenchAppBuilder::new("com.example.prod", "My Product")
//!         .with_backend_label("egui")
//!         .with_workbench(my_workbench)
//!         .with_plugin_stage(workbench_python::plugin_stage(
//!             workbench_core::cli::default_plugin_dirs("com.example.prod")))
//!         .create_default_document(true);
//!     let builder = cli.apply(builder);
//!
//!     match cli.mode {
//!         CliMode::Selfcheck => workbench_core::selfcheck::exit_with(&mut builder.build().unwrap()),
//!         CliMode::CrashProbe => workbench_core::cli::crash_probe_exit(|| make_builder()),
//!         CliMode::Run => { my_frontend::run(builder, "My Product").unwrap(); std::process::exit(0); }
//!     }
//! }
//! ```


use std::path::PathBuf;

use crate::WorkbenchAppBuilder;

/// The parsed command line.
#[derive(Clone, Debug)]
pub struct Cli {
    /// The run mode.
    pub mode: CliMode,
    /// Seconds for `--smoke [secs]`; Some only when `--smoke` was given.
    pub smoke_secs: Option<f32>,
}

/// The run mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliMode {
    /// 运行 GUI（默认）。
    Run,
    /// 无头自检。
    Selfcheck,
    /// 崩溃恢复探针（由 selfcheck 以子进程方式调用）。
    CrashProbe,
}

impl Cli {
    /// 从进程环境解析。
    pub fn from_env() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mode = args.get(1).cloned().unwrap_or_default();
        match mode.as_str() {
            "--selfcheck" => Self { mode: CliMode::Selfcheck, smoke_secs: None },
            "--selfcheck-crash" => Self { mode: CliMode::CrashProbe, smoke_secs: None },
            "--smoke" => Self {
                mode: CliMode::Run,
                smoke_secs: Some(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.5)),
            },
            _ => Self { mode: CliMode::Run, smoke_secs: None },
        }
    }

    /// Apply the smoke configuration to the assembly builder.
    pub fn apply(&self, builder: WorkbenchAppBuilder) -> WorkbenchAppBuilder {
        match self.smoke_secs {
            Some(secs) => builder.smoke(secs),
            None => builder,
        }
    }
}

/// Run the headless selfcheck and exit the process (exit code 0 = all passed).
pub fn selfcheck_exit(builder: WorkbenchAppBuilder) -> ! {
    let mut app = builder.build().expect("产品装配失败");
    std::process::exit(crate::selfcheck::run(&mut app));
}

/// Run the crash-recovery probe and exit the process (the phase-5 selfcheck's subprocess mode).
///
/// `make_builder` is called twice (session A produces the autosave, session B
/// verifies the recovery) and must use an isolated config directory (given by
/// the `WB_CRASH_DIR` environment variable).
pub fn crash_probe_exit(make_builder: impl Fn() -> WorkbenchAppBuilder) -> ! {
    let dir = PathBuf::from(
        std::env::var("WB_CRASH_DIR").expect("the WB_CRASH_DIR environment variable is required"),
    );
    let code = crate::selfcheck::crash_probe(
        make_builder().build().expect("session A assembly failed"),
        || make_builder().build().expect("session B assembly failed"),
        &dir.join("result.json"),
    );
    std::process::exit(code);
}

/// 插件发现目录的通用约定：当前工作目录的 `plugins/`（内置插件）
/// + 用户配置目录下 `<product_id>/plugins`（存在才生效）。
pub fn default_plugin_dirs(product_id: &str) -> Vec<PathBuf> {
    let mut dirs = vec![std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("plugins")];
    if let Some(home) = user_config_home() {
        let user_dir = home.join(product_id).join("plugins");
        if user_dir.is_dir() {
            dirs.push(user_dir);
        }
    }
    dirs
}

fn user_config_home() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
    }
}
