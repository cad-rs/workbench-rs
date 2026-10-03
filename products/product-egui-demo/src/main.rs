//! Example product: the egui frontend is selected at build time (design.md
//! acceptance items #1/#23: exactly one GUI per product).
//!
//! Demonstrates the standard downstream integration: assemble Workbench
//! modules + plugin directories; the CLI modes (run / --selfcheck /
//! --selfcheck-crash / --smoke [secs]) are hosted by the platform conventions.

use workbench_core::cli::{Cli, CliMode};

fn main() {
    env_logger::init(); // RUST_LOG=info prints wgpu/eframe diagnostics
    let cli = Cli::from_env();

    let builder = workbench_core::WorkbenchAppBuilder::new(
        "com.example.workbench-egui",
        "Workbench Demo - egui",
    )
    .with_backend_label("egui")
    .with_workbench(wb_example::ExampleWorkbench)
    .with_plugin_stage(workbench_python::plugin_stage(
        workbench_core::cli::default_plugin_dirs("com.example.workbench-egui"),
    ))
    .create_default_document(true)
    .open_on_start("example.welcome", false)
    .open_on_start("example.canvas", true);
    let builder = cli.apply(builder);

    match cli.mode {
        CliMode::Selfcheck => workbench_core::selfcheck::exit_with(&mut builder.build().unwrap()),
        CliMode::CrashProbe => workbench_core::cli::crash_probe_exit(|| {
            workbench_core::WorkbenchAppBuilder::new("selfcheck.crash", "Crash selfcheck")
                .with_workbench(wb_example::ExampleWorkbench)
                .create_default_document(true)
                .with_layout_path(
                    std::path::PathBuf::from(std::env::var("WB_CRASH_DIR").unwrap_or_default())
                        .join("layout.json"),
                )
        }),
        CliMode::Run => {
            if let Err(e) = workbench_ui_egui::run(builder, "Workbench Demo - egui") {
                eprintln!("run failed: {e:?}");
                std::process::exit(1);
            }
            // Platform async-executor threads are no longer needed after exit;
            // end the process directly for a clean shutdown.
            std::process::exit(0);
        }
    }
}
