//! Product command-line entry helpers (keeps downstream product `main`s tiny).
//!
//! Parses the common modes:
//! - (default) run the GUI;
//! - `--selfcheck`: headless selfcheck, process exit code 0/1;
//! - `--selfcheck-crash`: crash-recovery probe (invoked by the selfcheck as a
//!   subprocess, twice: `WB_CRASH_PHASE=A` crashes without running `Drop`,
//!   `WB_CRASH_PHASE=B` recovers; requires the `WB_CRASH_DIR` environment
//!   variable);
//! - `--smoke [secs]`: run for the given seconds and exit (default 2.5),
//!   for automated smoke tests.
//!
//! A typical product `main` (full example in the README "Integration guide"):
//!
//! ```text
//! fn main() {
//!     let cli = workbench_core::cli::from_env();
//!     let builder = workbench_core::WorkbenchAppBuilder::new("com.example.prod", "My Product")
//!         .with_backend_label("gpui")
//!         .with_workbench(my_workbench)
//!         .with_plugin_stage(workbench_rhai::plugin_stage(
//!             workbench_core::cli::default_plugin_dirs("com.example.prod")))
//!         .create_default_document(true);
//!     let builder = cli.apply(builder);
//!
//!     match cli.mode {
//!         CliMode::Selfcheck => workbench_core::selfcheck::exit_with(&mut builder.build().unwrap()),
//!         CliMode::CrashProbe => std::process::exit(workbench_core::cli::crash_probe_exit(make_builder)),
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
    /// Run the GUI (the default).
    Run,
    /// Headless selfcheck.
    Selfcheck,
    /// Crash-recovery probe (invoked by the selfcheck as a subprocess).
    CrashProbe,
}

impl Cli {
    /// Parse from the process environment.
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
    let mut app = builder.build().expect("product assembly failed");
    std::process::exit(crate::selfcheck::run(&mut app));
}

/// Run one phase of the crash-recovery probe (the phase-5 selfcheck's
/// subprocess mode) and return the process exit code.
///
/// The probe is a real crash simulation split across three subprocess launches:
///
/// - phase A (`WB_CRASH_PHASE=A`, the default): session A gives the product's
///   own default document a probe title, makes it dirty, autosaves it, then
///   exits **without running `Drop`** — so the `session.clean` marker is never
///   written.
/// - phase B (`WB_CRASH_PHASE=B`): session B builds, which detects the missing
///   marker and recovers the autosaved document, then verifies the recovered
///   document's content against what phase A persisted. It leaves a fresh dirty
///   document behind and returns normally, so `Drop` writes the marker.
/// - phase C (`WB_CRASH_PHASE=C`): the marker is present, so `build()` must not
///   recover even though the autosave directory still holds files.
///
/// `make_builder` must assemble the product normally (so its document types are
/// registered) with an isolated config directory given by `WB_CRASH_DIR`.
pub fn crash_probe_exit(make_builder: impl Fn() -> WorkbenchAppBuilder) -> i32 {
    let dir = PathBuf::from(
        std::env::var("WB_CRASH_DIR").expect("the WB_CRASH_DIR environment variable is required"),
    );
    let phase = std::env::var("WB_CRASH_PHASE").unwrap_or_default();
    match phase.as_str() {
        "B" => crate::selfcheck::crash_probe_recover(
            make_builder().build().expect("session B assembly failed"),
            &dir,
        ),
        "C" => crate::selfcheck::crash_probe_clean(
            make_builder().build().expect("session C assembly failed"),
            &dir,
        ),
        // Session A: the runtime is dropped by `std::process::exit`, i.e. not at
        // all — no clean marker is written.
        _ => crate::selfcheck::crash_probe_crash(
            make_builder().build().expect("session A assembly failed"),
            &dir,
        ),
    }
}

/// The common plugin discovery directories: `plugins/` under the current working directory
/// (built-in plugins) + `<product_id>/plugins` under the user config directory (only if it
/// exists).
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

/// The per-user configuration home: `%APPDATA%` on Windows, `~/.config` elsewhere.
pub(crate) fn user_config_home() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
    }
}
