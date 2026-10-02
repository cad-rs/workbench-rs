//! 产品命令行入口辅助（让下游产品的 `main` 保持极简）。
//!
//! 统一解析三类模式：
//! - （默认）运行 GUI；
//! - `--selfcheck`：无头自检，进程退出码 0/1；
//! - `--selfcheck-crash`：崩溃恢复探针（由 selfcheck 以子进程方式调用，
//!   依赖 `WB_CRASH_DIR` 环境变量）；
//! - `--smoke [秒]`：运行指定秒数后自动退出（默认 2.5 秒），自动化冒烟用。
//!
//! 典型产品 `main`（完整示例见 README「集成指南」）：
//!
//! ```text
//! fn main() {
//!     let cli = workbench_core::cli::from_env();
//!     let builder = workbench_core::WorkbenchAppBuilder::new("com.example.prod", "我的产品")
//!         .with_backend_label("egui")
//!         .with_workbench(my_workbench)
//!         .with_plugin_stage(workbench_python::plugin_stage(
//!             workbench_core::cli::default_plugin_dirs("com.example.prod")))
//!         .create_default_document(true);
//!     let builder = cli.apply(builder);
//!
//!     match cli.mode {
//!         CliMode::Selfcheck => workbench_core::selfcheck::exit_with(&mut builder.build().unwrap()),
//!         CliMode::CrashProbe => workbench_core::cli::crash_probe_exit(|| builder_like()),
//!         CliMode::Run => { my_frontend::run(builder, "我的产品").unwrap(); std::process::exit(0); }
//!     }
//! }
//! ```

use std::path::PathBuf;

use crate::WorkbenchAppBuilder;

/// CLI 解析结果。
#[derive(Clone, Debug)]
pub struct Cli {
    /// 运行模式。
    pub mode: CliMode,
    /// `--smoke [秒]` 的秒数；仅在 `--smoke` 时为 Some。
    pub smoke_secs: Option<f32>,
}

/// 运行模式。
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

    /// 把 smoke 配置应用到装配器。
    pub fn apply(&self, builder: WorkbenchAppBuilder) -> WorkbenchAppBuilder {
        match self.smoke_secs {
            Some(secs) => builder.smoke(secs),
            None => builder,
        }
    }
}

/// 无头自检并退出进程（退出码 0 = 全部通过）。
pub fn selfcheck_exit(builder: WorkbenchAppBuilder) -> ! {
    let mut app = builder.build().expect("产品装配失败");
    std::process::exit(crate::selfcheck::run(&mut app));
}

/// 崩溃恢复探针并退出进程（阶段 5 自检的子进程模式）。
///
/// `make_builder` 会被调用两次（会话 A 产生自动保存 + 会话 B 验证恢复），
/// 必须使用独立的配置目录（由 `WB_CRASH_DIR` 环境变量指定）。
pub fn crash_probe_exit(make_builder: impl Fn() -> WorkbenchAppBuilder) -> ! {
    let dir = PathBuf::from(
        std::env::var("WB_CRASH_DIR").expect("需要 WB_CRASH_DIR 环境变量"),
    );
    let code = crate::selfcheck::crash_probe(
        make_builder().build().expect("会话 A 装配失败"),
        || make_builder().build().expect("会话 B 装配失败"),
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
