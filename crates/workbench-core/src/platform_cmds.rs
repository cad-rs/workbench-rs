//! 平台内置命令（design.md §12.1 文档生命周期 + §12.2 撤销重做）。
//!
//! 命令通过 AppServices 门面操作平台，不触碰 GUI；快捷键绑定在这里集中登记。

use workbench_api as api;
use api::{CommandArgs, CommandCtx, CommandDef, CommandResult, Hotkey, KeyCode, Registry};

pub const NEW_DOCUMENT: &str = "app.new_document";
pub const OPEN_DOCUMENT: &str = "app.open_document";
pub const SAVE_DOCUMENT: &str = "app.save_document";
pub const SAVE_DOCUMENT_AS: &str = "app.save_document_as";
pub const UNDO: &str = "app.undo";
pub const REDO: &str = "app.redo";
pub const CLOSE_ACTIVE_VIEW: &str = "app.close_active_view";
pub const TOGGLE_PANEL_PREFIX: &str = "app.toggle_panel.";

fn all_extensions(ctx: &mut CommandCtx) -> Vec<String> {
    ctx.host()
        .docs
        .types()
        .iter()
        .flat_map(|t| t.extensions.iter().cloned())
        .collect()
}

fn preferred_view_of(ctx: &mut CommandCtx, doc_id: api::DocumentId) -> Option<api::ViewTypeId> {
    let host = ctx.host();
    let doc = host.docs.get(doc_id)?;
    host.docs.type_of(&doc.type_id).and_then(|t| t.open_view.clone())
}

pub fn register(registry: &mut Registry, panel_ids: &[(String, String)]) {
    // ---- 平台面板（阶段 5：诊断/插件管理） ----
    registry.register_panel(api::PanelDef::new(
        "app.panel.diagnostics",
        "诊断",
        api::DockArea::Right,
        320.0,
        2,
        || Box::new(crate::platform_panels::DiagnosticsPanel),
    ));
    registry.register_panel(api::PanelDef::new(
        "app.panel.plugins",
        "插件",
        api::DockArea::Right,
        320.0,
        3,
        || Box::new(crate::platform_panels::PluginsPanel),
    ));

    // ---- 插件管理命令 ----
    registry.register_command(
        CommandDef::sync(
            "app.plugins.disable",
            "禁用插件",
            |ctx: &mut CommandCtx, args: &CommandArgs| {
                let Some(id) = args.get("id") else {
                    return CommandResult::failed("缺少参数 id");
                };
                match ctx.disable_plugin(id) {
                    Ok(removed) => {
                        CommandResult::done_with(format!("已禁用 {id}（移除 {} 条命令）", removed.len()))
                    }
                    Err(e) => CommandResult::failed(e),
                }
            },
        )
        .enabled_when(|_| true),
    );

    // ---- 自动保存命令 ----
    registry.register_command(
        CommandDef::sync(
            "app.autosave.now",
            "立即自动保存",
            |ctx: &mut CommandCtx, _args: &CommandArgs| match ctx.autosave_now() {
                Ok(n) => CommandResult::done_with(format!("已自动保存 {n} 个文档")),
                Err(e) => CommandResult::failed(e),
            },
        ),
    );

    // ---- 文档生命周期命令 ----    // 新建文档
    registry.register_command(
        CommandDef::sync(NEW_DOCUMENT, "新建文档", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(type_id) = ctx.host().docs.types().first().map(|t| t.id.clone()) else {
                return CommandResult::failed("产品没有注册任何文档类型");
            };
            let count = ctx.host().docs.docs().count();
            let title = if count == 0 {
                "未命名".to_string()
            } else {
                format!("未命名{}", count + 1)
            };
            let doc_id = match ctx.host().docs.create(&type_id, &title) {
                Ok(id) => id,
                Err(e) => return CommandResult::failed(e),
            };
            ctx.host().docs.set_active(Some(doc_id));
            if let Some(vt) = preferred_view_of(ctx, doc_id) {
                if let Err(e) = ctx.open_view(vt.as_str(), true) {
                    ctx.host().log.warn(format!("打开视图失败: {e}"));
                }
            }
            ctx.host().log.info(format!("已新建文档「{title}」"));
            CommandResult::done()
        })
        .hotkey(Hotkey::new(KeyCode::N).ctrl(true)),
    );

    // 打开文档
    registry.register_command(
        CommandDef::sync(OPEN_DOCUMENT, "打开文档…", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let exts = all_extensions(ctx);
            let Some(path) = crate::dialog::pick_open_file(&exts) else {
                return CommandResult::done_with("已取消");
            };
            open_path(ctx, &path)
        })
        .hotkey(Hotkey::new(KeyCode::O).ctrl(true)),
    );

    // 保存
    registry.register_command(
        CommandDef::sync(SAVE_DOCUMENT, "保存", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("没有活动文档");
            };
            let has_path = ctx
                .host()
                .docs
                .get(doc_id)
                .map(|d| d.path.is_some())
                .unwrap_or(false);
            if has_path {
                match ctx.host().docs.save(doc_id) {
                    Ok(path) => {
                        ctx.host().log.info(format!("已保存到 {}", path.display()));
                        CommandResult::done()
                    }
                    Err(e) => CommandResult::failed(e),
                }
            } else {
                save_as_flow(ctx, doc_id)
            }
        })
        .hotkey(Hotkey::new(KeyCode::S).ctrl(true))
        .enabled_when(|snap| snap.active_document.is_some()),
    );

    // 另存为
    registry.register_command(
        CommandDef::sync(SAVE_DOCUMENT_AS, "另存为…", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("没有活动文档");
            };
            save_as_flow(ctx, doc_id)
        })
        .hotkey(Hotkey::new(KeyCode::S).ctrl(true).shift(true))
        .enabled_when(|snap| snap.active_document.is_some()),
    );

    // 撤销 / 重做
    registry.register_command(
        CommandDef::sync(UNDO, "撤销", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("没有活动文档");
            };
            match ctx.host().docs.undo(doc_id) {
                Ok(label) => {
                    ctx.host().log.info(format!("已撤销「{label}」"));
                    CommandResult::done_with(label)
                }
                Err(e) => CommandResult::failed(e),
            }
        })
        .hotkey(Hotkey::new(KeyCode::Z).ctrl(true))
        .enabled_when(|snap| {
            snap.active_document.as_ref().map(|d| d.can_undo).unwrap_or(false)
        }),
    );

    registry.register_command(
        CommandDef::sync(REDO, "重做", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("没有活动文档");
            };
            match ctx.host().docs.redo(doc_id) {
                Ok(label) => {
                    ctx.host().log.info(format!("已重做「{label}」"));
                    CommandResult::done_with(label)
                }
                Err(e) => CommandResult::failed(e),
            }
        })
        .hotkey(Hotkey::new(KeyCode::Y).ctrl(true))
        .enabled_when(|snap| {
            snap.active_document.as_ref().map(|d| d.can_redo).unwrap_or(false)
        }),
    );

    // 关闭活动中央标签
    registry.register_command(
        CommandDef::sync(CLOSE_ACTIVE_VIEW, "关闭当前标签", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            match ctx.close_active_view() {
                Ok(_) => CommandResult::done(),
                Err(e) => CommandResult::failed(e),
            }
        })
        .hotkey(Hotkey::new(KeyCode::W).ctrl(true)),
    );

    // 每个面板的显示/隐藏命令 + “窗口”工具栏组（含平台自己的面板）
    let mut all_panels: Vec<(String, String)> = panel_ids.to_vec();
    all_panels.push(("app.panel.diagnostics".to_string(), "诊断".to_string()));
    all_panels.push(("app.panel.plugins".to_string(), "插件".to_string()));
    let mut window_group = api::ToolbarGroup::new("app.window", "窗口");
    for (id, title) in &all_panels {
        let panel_id = id.clone();
        let pid = panel_id.clone();
        let title = title.clone();
        registry.register_command(
            CommandDef::sync(
                format!("{TOGGLE_PANEL_PREFIX}{id}"),
                format!("显示/隐藏 {title}"),
                move |ctx: &mut CommandCtx, _args: &CommandArgs| match ctx.toggle_panel(&pid) {
                    Ok(_) => CommandResult::done(),
                    Err(e) => CommandResult::failed(e),
                },
            ),
        );
        window_group
            .items
            .push(api::ToolbarItem::labeled(format!("{TOGGLE_PANEL_PREFIX}{id}"), title));
    }
    registry.add_toolbar_group(window_group);
}

/// 打开文件路径（打开命令共用流程）。
pub fn open_path(ctx: &mut CommandCtx, path: &std::path::Path) -> CommandResult {
    let doc_id = match ctx.host().docs.open_file(path) {
        Ok(id) => id,
        Err(e) => return CommandResult::failed(e),
    };
    ctx.host().docs.set_active(Some(doc_id));
    ctx.host().log.info(format!("已打开 {}", path.display()));
    if let Some(vt) = preferred_view_of(ctx, doc_id) {
        if let Err(e) = ctx.open_view(vt.as_str(), true) {
            ctx.host().log.warn(format!("打开视图失败: {e}"));
        }
    }
    CommandResult::done()
}

/// 运行时直接打开路径（selfcheck 使用，不经对话框）。
pub fn open_path_runtime(
    app: &mut crate::app::AppRuntime,
    path: &std::path::Path,
) -> Result<(), String> {
    use api::AppServices as _;
    let doc_id = app.host.docs.open_file(path)?;
    app.host.docs.set_active(Some(doc_id));
    app.host.log.info(format!("已打开 {}", path.display()));
    let view = app
        .host
        .docs
        .get(doc_id)
        .and_then(|d| app.host.docs.type_of(&d.type_id).and_then(|t| t.open_view.clone()));
    if let Some(vt) = view {
        app.open_view(vt.as_str(), true)?;
    }
    Ok(())
}

fn save_as_flow(ctx: &mut CommandCtx, doc_id: api::DocumentId) -> CommandResult {
    let (title, exts) = {
        let host = ctx.host();
        match host.docs.get(doc_id) {
            Some(doc) => {
                let exts = host
                    .docs
                    .type_of(&doc.type_id)
                    .map(|t| t.extensions.clone())
                    .unwrap_or_default();
                (doc.title.clone(), exts)
            }
            None => return CommandResult::failed("文档不存在"),
        }
    };
    let default_name = if title.contains('.') {
        title
    } else {
        format!("{}.{}", title, exts.first().map(|s| s.as_str()).unwrap_or("dat"))
    };
    let Some(path) = crate::dialog::pick_save_file(&default_name, &exts) else {
        return CommandResult::done_with("已取消");
    };
    match ctx.host().docs.save_as(doc_id, &path) {
        Ok(path) => {
            ctx.host().log.info(format!("已另存为 {}", path.display()));
            CommandResult::done()
        }
        Err(e) => CommandResult::failed(e),
    }
}
