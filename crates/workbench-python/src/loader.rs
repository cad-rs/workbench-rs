//! 插件发现与加载（design.md §11.5：宿主负责发现、加载、初始化、错误隔离）。

use std::path::Path;

use pyo3::prelude::*;
use pyo3::types::PyDict;

use workbench_api::{CommandDef, Registry, TaskFailure, TaskSpec};

use crate::host::{args_to_dict, Collected, HostApi, PluginCancelled, PyTask};
use crate::manifest::{self, PluginManifest};

/// 遍历发现目录，收集含 `plugin.toml` 的插件目录（按目录名排序，保证确定性）。
pub fn discover(dirs: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut subdirs: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect();
        subdirs.sort();
        for sub in subdirs {
            if sub.join("plugin.toml").is_file() {
                found.push(sub);
            }
        }
    }
    found
}

/// 把一个插件目录装载进注册表。失败被隔离：写 `plugins_rejected`，不影响其他插件。
fn load_one(registry: &mut Registry, dir: &Path) {
    let dir_name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.display().to_string());

    // 清单
    let manifest: PluginManifest = match manifest::parse(&dir.join("plugin.toml")) {
        Ok(m) => m,
        Err(e) => {
            registry.plugins_rejected.push((dir_name, e));
            return;
        }
    };
    let plugin_id = manifest.plugin.id.clone();

    // ID 唯一性
    if registry.plugins_loaded.iter().any(|id| id == &plugin_id) {
        registry
            .plugins_rejected
            .push((plugin_id, "插件 ID 重复（后发现的被跳过）".to_string()));
        return;
    }

    // 入口
    let (entry_file, entry_fn) = match manifest::split_entry_point(&manifest.plugin.entry_point) {
        Ok(x) => x,
        Err(e) => {
            registry.plugins_rejected.push((plugin_id, e));
            return;
        }
    };
    let entry_path = dir.join(&entry_file);

    // 加载执行：Python 回调期间只操作收集器，返回后统一落盘
    let collected = std::sync::Arc::new(std::sync::Mutex::new(Collected::default()));
    let result: Result<(), String> = Python::attach(|py| -> Result<(), String> {
        let module = import_module(py, &plugin_id, &entry_path)
            .map_err(|e| format!("加载 {} 失败: {e}", entry_path.display()))?;
        let func = module
            .getattr(&entry_fn)
            .map_err(|e| format!("入口函数 `{entry_fn}` 不存在: {e}"))?;
        let api = Py::new(
            py,
            HostApi {
                plugin_id: plugin_id.clone(),
                collected: collected.clone(),
            },
        )
        .map_err(|e| format!("创建 HostApi 失败: {e}"))?;
        func.call1((api,))
            .map_err(|e| format!("register() 执行失败: {e}"))?;
        Ok(())
    });

    if let Err(e) = result {
        registry.plugins_rejected.push((plugin_id, e));
        // 失败插件的半提交请求一并丢弃（收集器内容尚未落盘）
        return;
    }

    // 落盘：命令 + 工具栏 + 提示
    let mut collected = collected.lock().unwrap_or_else(|p| p.into_inner());
    for cmd in collected.commands.drain(..) {
        let def = if cmd.background {
            build_background_command(&cmd.id, &cmd.title, cmd.callback)
        } else {
            build_sync_command(&cmd.id, &cmd.title, cmd.callback)
        };
        registry.register_command(def.from_source(plugin_id.clone()));
    }
    for (group, command, label) in collected.toolbar.drain(..) {
        let group_id = workbench_api::ToolbarGroupId::new(group.clone());
        let item = match label {
            Some(l) => workbench_api::ToolbarItem::labeled(command, l),
            None => workbench_api::ToolbarItem::command(command),
        };
        // 受控贡献：向既有组追加，或创建插件自有组（§7.3）
        if let Some(existing) = registry.toolbar.iter_mut().find(|g| g.id == group_id) {
            existing.items.push(item);
        } else {
            registry.toolbar.push(
                workbench_api::ToolbarGroup::new(group.clone(), group)
                    .from_source(plugin_id.clone())
                    .items(vec![item]),
            );
        }
    }
    for note in collected.notes.drain(..) {
        registry.notes.push(note);
    }
    registry.plugins_loaded.push(plugin_id);
}

fn build_sync_command(id: &str, title: &str, callback: Py<PyAny>) -> CommandDef {
    let title = title.to_string();
    CommandDef::sync(id, title, move |_ctx, args| {
        let args = args.clone();
        // GIL 说明：同步插件命令在 UI 线程执行 Python；CPython 周期性让出 GIL，
        // 后台 Python 任务不会长时间阻塞 UI（复杂/长任务应使用 background 档）。
        Python::attach(|py| {
            let dict = args_to_dict(py, &args);
            match callback.bind(py).call1((dict,)) {
                Ok(ret) => match ret.extract::<Option<String>>() {
                    Ok(Some(note)) => workbench_api::CommandResult::done_with(note),
                    Ok(None) => workbench_api::CommandResult::done(),
                    Err(e) => workbench_api::CommandResult::failed(format!("返回值解析失败: {e}")),
                },
                Err(e) => workbench_api::CommandResult::failed(format!("Python 异常: {e}")),
            }
        })
    })
}

fn build_background_command(id: &str, title: &str, callback: Py<PyAny>) -> CommandDef {
    let title_for_spec = title.to_string();
    let title_for_job = title.to_string();
    let callback = std::sync::Arc::new(callback);
    CommandDef::background(id, title_for_spec, move |_ctx| {
        let title = title_for_job.clone();
        let callback = std::sync::Arc::clone(&callback);
        Ok(TaskSpec {
            title,
            cancellable: true,
            input: Box::new(()),
            job: Box::new(move |task: &mut workbench_api::TaskCtx| {
                let handle = task.progress_handle();
                let callback = std::sync::Arc::clone(&callback);
                // 在任务线程获取 GIL 执行插件回调
                Python::attach(|py| -> Result<Option<workbench_api::DocCommit>, TaskFailure> {
                    let py_task: Py<PyAny> = Py::new(py, PyTask { handle })
                        .map_err(|e| TaskFailure::Error(format!("创建 task 对象失败: {e}")))?
                        .into();
                    let dict = PyDict::new(py);
                    match callback.bind(py).call1((py_task, dict)) {
                        Ok(_) => Ok(None),
                        Err(e) => {
                            if e.is_instance_of::<PluginCancelled>(py) {
                                Err(TaskFailure::Cancelled)
                            } else {
                                Err(TaskFailure::Error(format!("Python 异常: {e}")))
                            }
                        }
                    }
                })
            }),
        })
    })
}

/// 通过 importlib 从文件路径导入模块。
fn import_module<'py>(
    py: Python<'py>,
    name: &str,
    path: &Path,
) -> PyResult<pyo3::Bound<'py, PyAny>> {
    let path_str = path
        .to_str()
        .ok_or_else(|| pyo3::exceptions::PyException::new_err("插件路径非 UTF-8"))?;
    let util = py.import("importlib.util")?;
    let spec = util.call_method1("spec_from_file_location", (name, path_str))?;
    if spec.is_none() {
        return Err(pyo3::exceptions::PyException::new_err(format!(
            "无法为 {path_str} 创建导入 spec"
        )));
    }
    let module = util.call_method1("module_from_spec", (&spec,))?;
    let loader = spec.getattr("loader")?;
    loader.call_method1("exec_module", (&module,))?;
    Ok(module)
}

/// 插件装载阶段（供 `WorkbenchAppBuilder::with_plugin_stage`）。
///
/// 对每个发现目录依次尝试装载；清单非法、API 版本不兼容、入口缺失或
/// register() 异常都会被记录为 `plugins_rejected`（验收 #19/#20），应用继续运行。
pub fn plugin_stage(dirs: Vec<std::path::PathBuf>) -> workbench_core::PluginStage {
    Box::new(move |registry: &mut Registry| {
        let mut candidates = discover(&dirs);
        candidates.sort();
        for dir in candidates {
            load_one(registry, &dir);
        }
    })
}
