//! Example product: assembled on workbench-rs with the gpui frontend and the
//! gpui-rhai script plugin host (design.md acceptance items #1/#23: exactly
//! one GUI per product).

use workbench_api as wb;
use workbench_core::cli::{Cli, CliMode};

/// product-demo 的装配 workbench：示例模块 + GPU 视图演示。
/// GPU 视图类型由 workbench-ui 的 `gpu_view` 模块提供（Workbench 模块层
/// 不接触 GUI，故视图注册在此追加，而不是放进 wb-example）。
struct DemoWorkbench;

impl wb::Workbench for DemoWorkbench {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("product-demo")
    }

    fn init(&self, reg: &mut wb::Registry) {
        wb_example::ExampleWorkbench.init(reg);
        workbench_ui::gpu_view::register_views(reg);
    }
}

fn main() {
    env_logger::init();
    let cli = Cli::from_env();

    let builder = workbench_core::WorkbenchAppBuilder::new(
        "com.example.workbench",
        "Workbench Demo",
    )
    .with_backend_label("gpui")
    .with_workbench(DemoWorkbench)
    .with_assets(wb_example::example_assets())
    .with_plugin_stage(workbench_rhai::plugin_stage(
        workbench_core::cli::default_plugin_dirs("com.example.workbench"),
    ))
    .create_default_document(true)
    .open_on_start("example.welcome", false)
    .open_on_start("example.canvas", true)
    // GPU 视口演示（gpui-wgpu 参考实现移植）：排在欢迎页与画布之后
    .open_on_start("example.gpu3d", false)
    .open_on_start("example.gpu_top", false);
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
            if let Err(e) = workbench_ui::run(builder, "Workbench Demo") {
                eprintln!("run failed: {e}");
                std::process::exit(1);
            }
            // Platform async-executor threads are no longer needed after exit;
            // end the process directly for a clean shutdown.
            std::process::exit(0);
        }
    }
}
