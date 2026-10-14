//! 插件发现与装载：目录扫描 → 清单校验 → 脚本 `register()` → Registry 落盘。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use workbench_api as api;
use workbench_api::{CommandDef, Registry};

use crate::host::{build_load_engine, call_sync_callback, run_background_callback, RhaiProgram, SharedCollected};
use crate::manifest::{parse as parse_manifest, split_entry_point, PluginManifest};

/// 扫描插件目录（每个一级子目录含 `plugin.toml` 即为插件，按名排序）。
pub fn discover(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("plugin.toml").is_file() {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    candidates
}

/// 装载单个插件：失败通过 `registry` 记录拒绝原因，不中断其它插件。
fn load_one(registry: &mut Registry, dir: &Path) {
    let dir_name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let Ok(text) = std::fs::read_to_string(dir.join("plugin.toml")) else {
        registry
            .plugins_rejected
            .push((dir_name, "plugin.toml 不可读".to_string()));
        return;
    };
    let manifest: PluginManifest = match parse_manifest(&text) {
        Ok(m) => m,
        Err(reason) => {
            registry.plugins_rejected.push((dir_name, reason));
            return;
        }
    };
    let plugin_id = manifest.plugin.id.clone();
    if registry.plugins_loaded.contains(&plugin_id) {
        registry.plugins_rejected.push((
            dir_name,
            format!("插件 ID 重复（已被 {plugin_id} 占用）"),
        ));
        return;
    }

    let (entry_file, entry_fn) = split_entry_point(&manifest.plugin.entry_point);
    let entry_path = dir.join(&entry_file);
    let Ok(source) = std::fs::read_to_string(&entry_path) else {
        registry
            .plugins_rejected
            .push((plugin_id, format!("入口脚本不可读: {}", entry_path.display())));
        return;
    };

    let collected: SharedCollected = Arc::new(Mutex::new(crate::host::Collected::default()));
    let engine = build_load_engine(dir.to_path_buf(), collected.clone());
    let ast = match engine.compile(&source) {
        Ok(ast) => ast,
        Err(e) => {
            registry
                .plugins_rejected
                .push((plugin_id, format!("脚本编译失败: {e}")));
            return;
        }
    };
    let _ = ast; // 编译期校验；命令回调在调用线程现场重建 Engine

    // 执行入口函数（缺省 `register`），脚本经宿主全局函数完成注册。
    let mut scope = rhai::Scope::new();
    if let Err(e) = engine.call_fn::<()>(&mut scope, &ast, &entry_fn, ()) {
        registry
            .plugins_rejected
            .push((plugin_id, format!("入口 `{entry_fn}` 执行失败: {e}")));
        return;
    }

    // 落盘：命令 + 工具栏 + 脚本面板 + 提示
    let program: RhaiProgram = Arc::new(source);
    let mut collected = collected.lock().unwrap_or_else(|p| p.into_inner());
    for cmd in collected.commands.drain(..) {
        let def = if cmd.background {
            build_background_command(&cmd.id, &cmd.title, &cmd.callback, program.clone())
        } else {
            build_sync_command(&cmd.id, &cmd.title, &cmd.callback, program.clone())
        };
        registry.register_command(def.from_source(plugin_id.clone()));
    }
    for (group, command, label) in collected.toolbar.drain(..) {
        let group_id = api::ToolbarGroupId::new(group.clone());
        let item = match label {
            Some(l) => api::ToolbarItem::labeled(command, l),
            None => api::ToolbarItem::command(command),
        };
        // 受控贡献：向既有组追加，或创建插件自有组（§7.3）
        if let Some(existing) = registry.toolbar.iter_mut().find(|g| g.id == group_id) {
            existing.items.push(item);
        } else {
            registry.toolbar.push(
                api::ToolbarGroup::new(group.clone(), group)
                    .from_source(plugin_id.clone())
                    .items(vec![item]),
            );
        }
    }
    for (id, title, script, area) in collected.panels.drain(..) {
        let area = match area.as_str() {
            "left" => api::DockArea::Left,
            "right" => api::DockArea::Right,
            "bottom" => api::DockArea::Bottom,
            "top" => api::DockArea::Top,
            _ => api::DockArea::Right,
        };
        let order = registry.panels.len() as i32;
        let def = api::PanelDef::new(
            id,
            title,
            area,
            300.0,
            order,
            || -> Box<dyn api::ViewInstance> {
                unreachable!("脚本面板不创建实例（内容由前端按 script 挂载）")
            },
        )
        .script(script.to_string_lossy().to_string());
        registry.register_panel(def);
    }
    for note in collected.notes.drain(..) {
        registry.notes.push(note);
    }
    registry.plugins_loaded.push(plugin_id);
}

fn build_sync_command(
    id: &str,
    title: &str,
    callback: &str,
    program: RhaiProgram,
) -> CommandDef {
    let callback = callback.to_string();
    CommandDef::sync(id, title, move |_ctx, args| {
        call_sync_callback(&program, &callback, args)
    })
}

fn build_background_command(
    id: &str,
    title: &str,
    callback: &str,
    program: RhaiProgram,
) -> CommandDef {
    let title_spec = title.to_string();
    let callback_owned = callback.to_string();
    CommandDef::background(id, title_spec.clone(), move |_ctx: &mut api::CommandCtx| {
        let title = title_spec.clone();
        let program = program.clone();
        let callback = callback_owned.clone();
        Ok(api::TaskSpec {
            title,
            cancellable: true,
            input: Box::new(()),
            job: Box::new(move |task: &mut api::TaskCtx| {
                run_background_callback(&program, &callback, task.progress_handle())
            }),
        })
    })
}

/// 插件装载阶段：装配期由 builder 调用，向 Registry 贡献命令/工具栏/面板。
/// 失败通过 `Registry` 记录，不中断装配（与其它 PluginStage 相同的隔离策略）。
pub fn plugin_stage(dirs: Vec<PathBuf>) -> workbench_core::PluginStage {
    Box::new(move |registry: &mut Registry| {
        for dir in discover(&dirs) {
            load_one(registry, &dir);
        }
    })
}
