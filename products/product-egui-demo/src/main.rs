//! 示例产品：构建时选定 egui 前端（design.md 验收 #1/#23：单一 GUI 前端）。
//!
//! 展示下游产品的标准整合方式：装配 Workbench 模块 + 插件目录，
//! CLI 模式（运行 / --selfcheck / --selfcheck-crash / --smoke [秒]）由平台约定托管。

use workbench_core::cli::{Cli, CliMode};

fn main() {
    env_logger::init(); // RUST_LOG=info 时输出 wgpu/eframe 诊断
    let cli = Cli::from_env();

    let builder = workbench_core::WorkbenchAppBuilder::new(
        "com.example.workbench-egui",
        "Workbench 演示 · egui",
    )
    .with_backend_label("egui")
    .with_workbench(wb_example::ExampleWorkbench)
    .with_plugin_stage(workbench_python::plugin_stage(workbench_core::cli::default_plugin_dirs(
        "com.example.workbench-egui",
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
            if let Err(e) = workbench_ui_egui::run(builder, "Workbench 演示 · egui") {
                eprintln!("运行失败: {e:?}");
                std::process::exit(1);
            }
            // 平台异步执行器线程在应用退出后不再需要；直接结束进程保证干净退出。
            std::process::exit(0);
        }
    }
}
