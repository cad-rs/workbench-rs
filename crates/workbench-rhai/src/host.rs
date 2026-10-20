//! 插件脚本的 Host API v1：脚本通过宿主注入的全局函数完成注册。
//!
//! ```rhai
//! // plugin.rhai
//! fn register() {
//!     register_command("rhai.hello", "插件问候", "hello");
//!     add_toolbar_item("plugin.rhai", "rhai.hello", "问候");
//! }
//! fn hello(args) { "来自 Rhai 的问候" }   // 返回字符串作为备注；返回 () 表示无备注
//! ```
//!
//! 线程模型：rhai 默认（非 `sync`）的 Engine/AST 不是 Send（gpui-rhai 也
//! 依赖这一点），因此命令回调不持有 Engine——只持有脚本源码（`Arc<String>`
//! 是 Send），调用时在当前线程现场构建 Engine 并编译。脚本都很小，
//! 构建成本在毫秒级。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use workbench_api::{CommandArgs, CommandResult, ProgressHandle};

/// 脚本取消哨兵：后台回调抛出含此标记的错误时映射为协作式取消。
pub const CANCEL_SENTINEL: &str = "__wb_cancelled__";

/// 脚本在 `register()` 里声明的一条命令。
pub struct CollectedCommand {
    pub id: String,
    pub title: String,
    pub callback: String,
    pub background: bool,
}

/// `register()` 期间的受控收集结果（返回装载方后统一落盘 Registry）。
#[derive(Default)]
pub struct Collected {
    pub commands: Vec<CollectedCommand>,
    pub toolbar: Vec<(String, String, Option<String>)>,
    /// (面板 id, 标题, 脚本绝对路径, 停靠区字符串)
    pub panels: Vec<(String, String, std::path::PathBuf, String)>,
    /// (视图 id, 标题, 脚本绝对路径)
    pub views: Vec<(String, String, std::path::PathBuf)>,
    pub notes: Vec<String>,
}

/// 共享收集桶（脚本全局函数写，装载线程读）。
pub type SharedCollected = Arc<Mutex<Collected>>;

/// 命令回调引用的脚本源码（Send，可跨线程进入任务闭包）。
pub type RhaiProgram = Arc<String>;

/// 装载期 Engine：注入 `register()` 需要的宿主全局函数。
/// 仅在装载线程使用，随装载结束丢弃。
pub fn build_load_engine(plugin_dir: std::path::PathBuf, collected: SharedCollected) -> rhai::Engine {
    let mut engine = rhai::Engine::new();

    // register_command(id, title, callback) — 同步命令（UI 线程执行）
    {
        let collected = collected.clone();
        engine.register_fn("register_command", move |id: &str, title: &str, callback: &str| {
            collected
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .commands
                .push(CollectedCommand {
                    id: id.into(),
                    title: title.into(),
                    callback: callback.into(),
                    background: false,
                });
        });
    }
    // register_background_command(id, title, callback) — 后台线程任务
    {
        let collected = collected.clone();
        engine.register_fn(
            "register_background_command",
            move |id: &str, title: &str, callback: &str| {
                collected
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .commands
                    .push(CollectedCommand {
                        id: id.into(),
                        title: title.into(),
                        callback: callback.into(),
                        background: true,
                    });
            },
        );
    }
    // add_toolbar_item(group, command, label)
    {
        let collected = collected.clone();
        engine.register_fn(
            "add_toolbar_item",
            move |group: &str, command: &str, label: &str| {
                collected
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .toolbar
                    .push((group.into(), command.into(), Some(label.into())));
            },
        );
    }
    // add_toolbar_item_no_label(group, command)
    {
        let collected = collected.clone();
        engine.register_fn("add_toolbar_item_no_label", move |group: &str, command: &str| {
            collected
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .toolbar
                .push((group.into(), command.into(), None));
        });
    }
    // register_panel(id, title, script, area) — 脚本 UI 面板（gpui-rhai 渲染）
    {
        let collected = collected.clone();
        let dir = plugin_dir.clone();
        engine.register_fn(
            "register_panel",
            move |id: &str, title: &str, script: &str, area: &str| {
                let area = area.trim().to_ascii_lowercase();
                if !matches!(area.as_str(), "left" | "right" | "bottom" | "top") {
                    return; // 未知停靠区：静默忽略
                }
                let mut c = collected.lock().unwrap_or_else(|p| p.into_inner());
                c.panels.push((id.into(), title.into(), dir.join(script), area));
            },
        );
    }
    // register_view(id, title, script) — script central view (gpui-rhai rendered)
    {
        let collected = collected.clone();
        let dir = plugin_dir;
        engine.register_fn(
            "register_view",
            move |id: &str, title: &str, script: &str| {
                let mut c = collected.lock().unwrap_or_else(|p| p.into_inner());
                c.views.push((id.into(), title.into(), dir.join(script)));
            },
        );
    }
    // log_info(msg)
    {
        let collected = collected.clone();
        engine.register_fn("log_info", move |message: &str| {
            collected
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .notes
                .push(message.into());
        });
    }

    engine
}

/// 后台任务句柄的 Rhai 包装（`fn progress_job(task, args)` 的 `task` 参数）。
#[derive(Clone)]
struct RhaiTask {
    handle: ProgressHandle,
}

/// 回调执行 Engine：注册后台回调需要的 `task` 对象方法。
/// 每次调用现场构建（不跨线程持有）。
fn build_exec_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();

    engine.register_type_with_name::<RhaiTask>("Task");
    engine.register_fn(
        "report",
        |task: &mut RhaiTask, fraction: rhai::FLOAT, stage: &str| {
            task.handle.report(Some(fraction as f32), Some(stage));
        },
    );
    engine.register_fn("report_progress", |task: &mut RhaiTask, fraction: rhai::FLOAT| {
        task.handle.report(Some(fraction as f32), None);
    });
    engine.register_fn("set_stage", |task: &mut RhaiTask, stage: &str| {
        task.handle.report(None, Some(stage));
    });
    engine.register_fn("log", |task: &mut RhaiTask, message: &str| {
        task.handle.log(message);
    });
    engine.register_fn("is_cancelled", |task: &mut RhaiTask| {
        task.handle.is_cancelled()
    });
    // 协作式取消检查点：已取消时抛出哨兵错误，任务循环据此退出。
    engine.register_fn(
        "check_cancelled",
        |task: &mut RhaiTask| -> Result<(), Box<rhai::EvalAltResult>> {
            if task.handle.is_cancelled() {
                Err(format!("{CANCEL_SENTINEL}: 已请求取消").into())
            } else {
                Ok(())
            }
        },
    );
    engine
}

/// `CommandArgs` → Rhai Map（字符串键值；复杂类型由命令脚本自行约定）。
pub fn args_to_map(args: &CommandArgs) -> rhai::Map {
    let mut map = rhai::Map::new();
    for (k, v) in &args.values {
        map.insert(k.clone().into(), v.clone().into());
    }
    map
}

/// 调用同步回调：脚本返回字符串作为备注，返回 `()` 表示无备注。
pub fn call_sync_callback(
    program: &RhaiProgram,
    callback: &str,
    args: &CommandArgs,
) -> CommandResult {
    let engine = build_exec_engine();
    let result = (|| -> Result<rhai::Dynamic, String> {
        let ast = engine.compile(&**program).map_err(|e| e.to_string())?;
        let mut scope = rhai::Scope::new();
        let map = args_to_map(args);
        engine
            .call_fn::<rhai::Dynamic>(&mut scope, &ast, callback, (map,))
            .map_err(|e| e.to_string())
    })();
    match result {
        Ok(value) => {
            if value.is_unit() {
                CommandResult::done()
            } else {
                match value.into_string() {
                    Ok(note) => CommandResult::done_with(note),
                    Err(_) => CommandResult::done(),
                }
            }
        }
        Err(e) => CommandResult::failed(format!("脚本回调 `{callback}` 失败: {e}")),
    }
}

/// 后台回调执行体（任务线程）。返回 `None` 表示不提交文档变更。
pub fn run_background_callback(
    program: &RhaiProgram,
    callback: &str,
    handle: ProgressHandle,
) -> Result<Option<workbench_api::DocCommit>, workbench_api::TaskFailure> {
    let engine = build_exec_engine();
    let run = (|| -> Result<rhai::Dynamic, String> {
        let ast = engine.compile(&**program).map_err(|e| e.to_string())?;
        let task = RhaiTask { handle };
        let mut scope = rhai::Scope::new();
        let map = rhai::Map::new();
        engine
            .call_fn::<rhai::Dynamic>(&mut scope, &ast, callback, (task, map))
            .map_err(|e| e.to_string())
    })();
    match run {
        Ok(_) => Ok(None),
        Err(e) => {
            if e.to_string().contains(CANCEL_SENTINEL) {
                Err(workbench_api::TaskFailure::Cancelled)
            } else {
                Err(workbench_api::TaskFailure::Error(format!(
                    "脚本任务 `{callback}` 失败: {e}"
                )))
            }
        }
    }
}

/// 供测试/工具使用的空参数表。
pub fn empty_map() -> rhai::Map {
    BTreeMap::new()
}
