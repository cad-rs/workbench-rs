//! 示例产品：构建时选定 gpui 前端（design.md 验收 #1/#23：单一 GUI 前端）。
//!
//! 与 egui 版唯一差异是前端适配器与后端标签——Workbench 与 CLI 完全一致。

use workbench_core::cli::{Cli, CliMode};

fn main() {
    env_logger::init();
    let cli = Cli::from_env();

    let builder = workbench_core::WorkbenchAppBuilder::new(
        "com.example.workbench-gpui",
        "Workbench 演示 · gpui",
    )
    .with_backend_label("gpui")
    .with_workbench(wb_example::ExampleWorkbench)
    .with_plugin_stage(workbench_python::plugin_stage(workbench_core::cli::default_plugin_dirs(
        "com.example.workbench-gpui",
    )))
    .create_default_document(true)
    .open_on_start("example.welcome", false)
    .open_on_start("example.canvas", true);
    let builder = cli.apply(builder);

    match cli.mode {
        CliMode::Selfcheck => workbench_core::selfcheck::exit_with(&mut builder.build().unwrap()),
        CliMode::CrashProbe => workbench_core::cli::crash_probe_exit(|| {
            workbench_core::WorkbenchAppBuilder::new("selfcheck.crash", "崩溃自检")
                .with_workbench(wb_example::ExampleWorkbench)
                .create_default_document(true)
                .with_layout_path(
                    std::path::PathBuf::from(std::env::var("WB_CRASH_DIR").unwrap_or_default())
                        .join("layout.json"),
                )
        }),
        CliMode::Run => {
            if let Err(e) = workbench_ui_gpui::run(builder, "Workbench 演示 · gpui") {
                eprintln!("运行失败: {e}");
                std::process::exit(1);
            }
            // 平台异步执行器线程在应用退出后不再需要；直接结束进程保证干净退出。
            std::process::exit(0);
        }
    }
}
