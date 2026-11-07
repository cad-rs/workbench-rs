//! 平台内置命令（design.md §12.1 文档生命周期 + §12.2 撤销重做）。
//!
//! 命令通过 AppServices 门面操作平台，不触碰 GUI；快捷键绑定在这里集中登记。

use workbench_api as api;
use api::{
    CommandArgs, CommandCtx, CommandDef, CommandResult, Hotkey, KeyCode, Registry, SettingKind,
    SettingScope, SettingSpec,
};

pub const NEW_DOCUMENT: &str = "app.new_document";
pub const OPEN_DOCUMENT: &str = "app.open_document";
pub const SAVE_DOCUMENT: &str = "app.save_document";
pub const SAVE_DOCUMENT_AS: &str = "app.save_document_as";
pub const UNDO: &str = "app.undo";
pub const REDO: &str = "app.redo";
pub const CLOSE_ACTIVE_VIEW: &str = "app.close_active_view";
pub const TOGGLE_PANEL_PREFIX: &str = "app.toggle_panel.";
pub const ABOUT: &str = "app.about";
pub const SETTINGS_SET: &str = "app.settings.set";
pub const SETTINGS_EDIT_BEGIN: &str = "app.settings.edit_begin";
pub const ACCOUNT_LOGIN: &str = "app.account.login";
pub const ACCOUNT_CANCEL_LOGIN: &str = "app.account.cancel_login";
pub const ACCOUNT_LOGOUT: &str = "app.account.logout";

/// Built-in locale tables. Convention: the source-language string (the one
/// hardcoded at the call site) is the key — `zh-CN` shows the original, `en`
/// maps it to English (Qt tr() style). Display sites call `host.tr(key)`;
/// a missing key renders the key itself, so untranslated strings degrade to
/// the source text instead of blanking the UI.
fn register_builtin_locales(registry: &mut Registry) {
    registry.register_locales(
        "en",
        [
            // Document lifecycle commands.
            ("新建文档", "New Document"),
            ("打开文档…", "Open Document…"),
            ("保存文档", "Save Document"),
            ("另存为…", "Save As…"),
            ("撤销", "Undo"),
            ("重做", "Redo"),
            ("关闭当前标签", "Close Tab"),
            ("折叠/展开 Ribbon", "Collapse/Expand Ribbon"),
            // Plugin & settings commands.
            ("禁用插件", "Disable Plugin"),
            ("立即自动保存", "Save All Now"),
            ("修改设置", "Change Setting"),
            ("关于", "About"),
            ("显示/隐藏 ", "Show/Hide "),
            // Ribbon groups & tabs.
            ("窗口", "View"),
            ("帮助", "Help"),
            ("主页", "Home"),
            ("常规", "General"),
            ("外观", "Appearance"),
            // Panel titles.
            ("诊断", "Diagnostics"),
            ("插件", "Plugins"),
            ("设置", "Settings"),
            ("欢迎", "Welcome"),
            // Settings panel rows.
            ("语言", "Language"),
            ("主题", "Theme"),
            ("深色", "Dark"),
            ("浅色", "Light"),
            ("强调色", "Accent color"),
            ("自动保存间隔（秒）", "Autosave interval (s)"),
            // Settings values / controls.
            ("开", "On"),
            ("关", "Off"),
            // Diagnostics panel.
            ("运行时", "Runtime"),
            ("产品", "Product"),
            ("（见窗口标题）", "(see window title)"),
            ("GUI 后端", "GUI backend"),
            ("（未标注）", "(unlabeled)"),
            ("运行时长", "Uptime"),
            ("秒", "s"),
            ("帧统计", "Frame stats"),
            ("ms/帧（最近 ", "ms/frame (recent "),
            ("文档与任务", "Documents & tasks"),
            ("文档数", "Documents"),
            ("未保存", "Unsaved"),
            ("个", ""),
            ("后台任务", "Background tasks"),
            ("运行 ", "running "),
            (" / 累计 ", " / total "),
            ("自动保存", "Autosave"),
            ("间隔", "Interval"),
            ("秒（settings.json 可调）", "s (tunable in settings.json)"),
            ("已自动保存", "Autosaved"),
            ("次", ""),
            // Plugins panel.
            ("没有已注册的插件", "No plugins registered"),
            ("已装载", "Loaded"),
            ("已禁用", "Disabled"),
            ("已拒绝", "Rejected"),
            ("禁用", "Disable"),
            // About dialog.
            ("版本", "Version"),
            ("主页", "Home"),
            ("支持", "Support"),
            ("关闭", "Close"),
            // Window-state log lines.
            ("已折叠", "Collapsed"),
            ("已展开", "Expanded"),
            // Status bar.
            ("文档：", "Doc: "),
            ("（无）", "(none)"),
            (" · 修订 r", " · rev r"),
            ("模式：", "Mode: "),
            ("取消", "Cancel"),
            // Account chip (Phase 3).
            ("登录", "Sign in"),
            ("登出", "Sign out"),
        ],
    );
}

/// Built-in setting specs: the settings panel renders rows from these and
/// `app.settings.set` validates writes against them.
fn register_builtin_settings(registry: &mut Registry) {
    registry.register_setting(SettingSpec {
        key: "ui.locale".into(),
        label: "语言".into(),
        group: "外观".into(),
        kind: SettingKind::Enum(vec!["zh-CN".into(), "en".into()]),
        default: serde_json::json!("zh-CN"),
        scope: SettingScope::User,
    });
    registry.register_setting(SettingSpec {
        key: "ui.theme".into(),
        label: "主题".into(),
        group: "外观".into(),
        kind: SettingKind::Enum(vec!["dark".into(), "light".into()]),
        default: serde_json::json!("dark"),
        scope: SettingScope::User,
    });
    // Brand accent (`#RRGGBB`), seeded from product.toml's branding.accent at
    // build time when the product ships one.
    registry.register_setting(SettingSpec {
        key: "ui.accent".into(),
        label: "强调色".into(),
        group: "外观".into(),
        kind: SettingKind::Str,
        default: serde_json::json!("#338CF2"),
        scope: SettingScope::User,
    });
    registry.register_setting(SettingSpec {
        key: "autosave.interval_secs".into(),
        label: "自动保存间隔（秒）".into(),
        group: "常规".into(),
        kind: SettingKind::Int { min: 5, max: 3600, step: 5 },
        default: serde_json::json!(30),
        scope: SettingScope::User,
    });
}

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
    let def = host.docs.type_of(&doc.type_id)?;
    let open_view = def.open_view?;
    open_view(doc.content.as_ref())
}

pub fn register(registry: &mut Registry, panel_ids: &[(String, String)]) {
    // The built-in platform panels (diagnostics / plugins) are view
    // implementations, so the frontend registers them before this runs; their
    // ids therefore already appear in `panel_ids` below.

    // ---- 平台内置设置项 ----
    register_builtin_settings(registry);

    // ---- 平台内置文案资源表（en） ----
    register_builtin_locales(registry);

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
        CommandDef::sync(NEW_DOCUMENT, "新建文档", |ctx: &mut CommandCtx, args: &CommandArgs| {
            // `type` names a specific document type (the welcome page offers one
            // entry per type); without it the product's default type is used.
            let wanted = args
                .get("type")
                .map(str::to_string)
                .or_else(|| ctx.host().ws.default_document_type.clone());
            let type_id = {
                let types = ctx.host().docs.types();
                wanted
                    .and_then(|id| types.iter().find(|t| t.id.0 == id).map(|t| t.id.clone()))
                    .or_else(|| types.first().map(|t| t.id.clone()))
            };
            let Some(type_id) = type_id else {
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
        .icon(crate::assets::icons::NEW)
        .hotkey(Hotkey::new(KeyCode::N).ctrl(true)),
    );

    // 打开文档
    registry.register_command(
        CommandDef::sync(OPEN_DOCUMENT, "打开文档…", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            request_open_dialog(ctx);
            CommandResult::done_with("已弹出文件对话框")
        })
        .icon(crate::assets::icons::OPEN)
        .hotkey(Hotkey::new(KeyCode::O).ctrl(true)),
    );

    // 保存
    registry.register_command(
        CommandDef::sync(SAVE_DOCUMENT, "保存", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("没有活动文档");
            };
            let has_file = ctx
                .host()
                .docs
                .get(doc_id)
                .is_some_and(|d| d.location.is_some());
            if has_file {
                save_document(ctx, doc_id, None)
            } else {
                request_save_as_dialog(ctx, doc_id)
            }
        })
        .icon(crate::assets::icons::SAVE)
        .hotkey(Hotkey::new(KeyCode::S).ctrl(true))
        .enabled_when(|snap| snap.active_document.is_some()),
    );

    // 另存为
    registry.register_command(
        CommandDef::sync(SAVE_DOCUMENT_AS, "另存为…", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("没有活动文档");
            };
            request_save_as_dialog(ctx, doc_id)
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
        .icon(crate::assets::icons::UNDO)
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
        .icon(crate::assets::icons::REDO)
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

    // Ribbon 折叠切换（Ctrl+F1，仅开启时生效，ribbon-bar.md §4.5）
    registry.register_command(
        CommandDef::sync(
            "app.toggle_ribbon_collapse",
            "折叠/展开 Ribbon",
            |ctx: &mut CommandCtx, _args: &CommandArgs| {
                let host = ctx.host();
                if !host.ws.ribbon_collapse_enabled {
                    return CommandResult::failed("Ribbon 折叠能力未开启");
                }
                host.ws.ribbon_collapsed = !host.ws.ribbon_collapsed;
                CommandResult::done_with(if host.ws.ribbon_collapsed {
                    "已折叠"
                } else {
                    "已展开"
                })
            },
        )
        .hotkey(Hotkey::new(KeyCode::F1).ctrl(true)),
    );

    // 设置写入：按注册的 spec 做类型校验与范围钳制，写入并立即落盘。
    // CommandArgs 是字符串表；按 spec 的类型把字符串解析成 Value 再校验。
    registry.register_command(
        CommandDef::sync(
            SETTINGS_SET,
            "修改设置",
            |ctx: &mut CommandCtx, args: &CommandArgs| {
                let Some(key) = args.get("key") else {
                    return CommandResult::failed("缺少参数 key");
                };
                let Some(raw) = args.get("value") else {
                    return CommandResult::failed("缺少参数 value");
                };
                let host = ctx.host();
                let Some(spec) = host.setting_specs.iter().find(|s| s.key == key).cloned() else {
                    return CommandResult::failed(format!("未注册的设置项: {key}"));
                };
                let parsed = match &spec.kind {
                    SettingKind::Bool => match raw {
                        "true" => serde_json::Value::Bool(true),
                        "false" => serde_json::Value::Bool(false),
                        _ => return CommandResult::failed(format!("{raw} 不是布尔值")),
                    },
                    SettingKind::Int { .. } => match raw.parse::<i64>() {
                        Ok(n) => serde_json::Value::Number(n.into()),
                        Err(_) => return CommandResult::failed(format!("{raw} 不是整数")),
                    },
                    SettingKind::Enum(_) | SettingKind::Str => {
                        serde_json::Value::String(raw.to_string())
                    }
                };
                let normalized = match spec.validate(parsed) {
                    Ok(v) => v,
                    Err(e) => return CommandResult::failed(e),
                };
                let host = ctx.host();
                match host.settings.set(&spec.key, normalized.clone()) {
                    Ok(()) => {
                        host.log.info(format!("[设置] {} = {normalized}", spec.key));
                        CommandResult::done_with(format!("{} = {normalized}", spec.label))
                    }
                    Err(e) => CommandResult::failed(e),
                }
            },
        )
        .enabled_when(|_| true),
    );

    // Settings text edit: seed the buffer from the current value and open
    // the frontend's input overlay (Enter commits via app.settings.set).
    registry.register_command(
        CommandDef::sync(
            SETTINGS_EDIT_BEGIN,
            "编辑设置",
            |ctx: &mut CommandCtx, args: &CommandArgs| {
                let Some(key) = args.get("key") else {
                    return CommandResult::failed("缺少参数 key");
                };
                let host = ctx.host();
                let Some(spec) = host.setting_specs.iter().find(|s| s.key == key).cloned() else {
                    return CommandResult::failed(format!("未注册的设置项: {key}"));
                };
                // Free-form strings are the only editable-by-hand kind for
                // now (bool/enum/int have dedicated controls).
                if !matches!(spec.kind, SettingKind::Str) {
                    return CommandResult::failed(format!("{} 不是文本设置项", spec.key));
                }
                let buffer = host
                    .settings
                    .get(&spec.key)
                    .cloned()
                    .unwrap_or_else(|| spec.effective_default())
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                host.ws.text_edit = Some(api::TextEdit::setting(spec.key.clone(), buffer));
                CommandResult::done()
            },
        )
        .enabled_when(|_| true),
    );

    // About 对话框：置位 workspace 标志，前端（workbench-ui）渲染对话框。
    registry.register_command(
        CommandDef::sync(ABOUT, "关于", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            ctx.host().ws.about_open = true;
            CommandResult::done()
        })
        .enabled_when(|_| true),
    );

    // 每个面板的显示/隐藏命令 + “窗口”工具栏组（含平台自己的面板）
    let all_panels: Vec<(String, String)> = panel_ids.to_vec();
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

    // “帮助”工具栏组：About 对话框入口（产品品牌信息）。
    let mut help_group = api::ToolbarGroup::new("app.help", "帮助");
    help_group
        .items
        .push(api::ToolbarItem::labeled(ABOUT, "关于"));
    registry.add_toolbar_group(help_group);
}

/// Open the document at `location`: its storage provider reads the file off
/// the UI thread, then the document is decoded, made active and shown in its
/// preferred view. Failures go to the log.
pub fn open_document(ctx: &mut CommandCtx, location: api::Location) {
    let type_id = match ctx.host().docs.type_for(&location) {
        Ok(type_id) => type_id,
        Err(e) => {
            ctx.host().log.error(e);
            return;
        }
    };
    ctx.read_file(location.clone(), Box::new(move |app, bytes| {
        let mut ctx = CommandCtx::new(app);
        let opened = bytes.and_then(|bytes| {
            let title = location.file_stem();
            ctx.host().docs.open_bytes(&type_id, title, Some(location.clone()), &bytes)
        });
        let doc_id = match opened {
            Ok(id) => id,
            Err(e) => {
                ctx.host().log.error(e);
                return;
            }
        };
        ctx.host().docs.set_active(Some(doc_id));
        ctx.host().log.info(format!("已打开 {location}"));
        if let Some(vt) = preferred_view_of(&mut ctx, doc_id) {
            if let Err(e) = ctx.open_view(vt.as_str(), true) {
                ctx.host().log.warn(format!("打开视图失败: {e}"));
            }
        }
    }));
}

/// Save a document to `target`, or to its own file when None. It is encoded
/// now and written off the UI thread; how the write went goes to the log.
pub fn save_document(
    ctx: &mut CommandCtx,
    doc_id: api::DocumentId,
    target: Option<api::Location>,
) -> CommandResult {
    let save_as = target.is_some();
    let (save, bytes) = match ctx.host().docs.begin_save(doc_id, target) {
        Ok(started) => started,
        Err(e) => return CommandResult::failed(e),
    };
    ctx.write_file(save.location.clone(), bytes, Box::new(move |app, written| {
        let host = app.host();
        match host.docs.finish_save(save, written) {
            Ok(location) if save_as => host.log.info(format!("已另存为 {location}")),
            Ok(location) => host.log.info(format!("已保存到 {location}")),
            Err(e) => host.log.error(e),
        }
    }));
    CommandResult::done()
}

/// 请平台在后台线程弹出“打开文档”对话框，选中后于 UI 线程打开。
///
/// 阻塞式 OS 模态循环绝不能在 UI 线程执行（会重入 gpui 窗口过程、楔死
/// 渲染循环），所以走 [`CommandCtx::request_file_dialog`]。
fn request_open_dialog(ctx: &mut CommandCtx) {
    let exts = all_extensions(ctx);
    ctx.request_file_dialog(exts, None, Box::new(|ctx, picked| {
        if let Some(location) = picked {
            open_document(&mut CommandCtx::new(ctx), location);
        }
    }));
}

/// 请平台在后台线程弹出“另存为”对话框，选中后于 UI 线程落盘。
fn request_save_as_dialog(ctx: &mut CommandCtx, doc_id: api::DocumentId) -> CommandResult {
    let (default_name, exts) = {
        let host = ctx.host();
        match host.docs.get(doc_id) {
            Some(doc) => {
                let exts = host
                    .docs
                    .type_of(&doc.type_id)
                    .map(|t| t.extensions.clone())
                    .unwrap_or_default();
                let default_name = if doc.title.contains('.') {
                    doc.title.clone()
                } else {
                    format!(
                        "{}.{}",
                        doc.title,
                        exts.first().map(|s| s.as_str()).unwrap_or("dat")
                    )
                };
                (default_name, exts)
            }
            None => return CommandResult::failed("文档不存在"),
        }
    };
    ctx.request_file_dialog(exts, Some(default_name), Box::new(move |ctx, picked| {
        let Some(location) = picked else {
            return;
        };
        let mut cmd = CommandCtx::new(ctx);
        if let CommandResult::Failed { error } = save_document(&mut cmd, doc_id, Some(location)) {
            cmd.host().log.error(error);
        }
    }));
    CommandResult::done_with("已弹出文件对话框")
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::CommandArgs;

    /// A minimal runtime (no product workbench): build() registers the
    /// platform commands and built-in setting specs.
    fn runtime() -> crate::AppRuntime {
        crate::WorkbenchAppBuilder::new("test.settings", "Settings test")
            .build()
            .expect("runtime builds")
    }

    #[test]
    fn edit_begin_opens_buffer_for_string_settings() {
        let mut rt = runtime();
        rt.host
            .settings
            .set("ui.accent", serde_json::json!("#112233"))
            .unwrap();
        let result = rt.execute_command(
            SETTINGS_EDIT_BEGIN,
            &CommandArgs::new().with("key", "ui.accent"),
        );
        assert!(result.is_ok(), "command succeeds: {result:?}");
        let edit = rt.host.ws.text_edit.as_ref().expect("edit open");
        assert_eq!(edit.key, "ui.accent");
        assert_eq!(edit.buffer, "#112233", "buffer seeded from current value");
    }

    #[test]
    fn edit_begin_rejects_non_string_settings() {
        let mut rt = runtime();
        let result = rt.execute_command(
            SETTINGS_EDIT_BEGIN,
            &CommandArgs::new().with("key", "ui.theme"),
        );
        assert!(rt.host.ws.text_edit.is_none(), "enum has its own control");
        let _ = result;
    }

    #[test]
    fn settings_set_validates_and_persists() {
        let mut rt = runtime();
        // Enum membership is validated.
        let bad = rt.execute_command(
            SETTINGS_SET,
            &CommandArgs::new()
                .with("key", "ui.theme")
                .with("value", "blue"),
        );
        assert!(!bad.is_ok(), "invalid enum rejected: {bad:?}");
        // Int values are clamped into the spec range.
        rt.execute_command(
            SETTINGS_SET,
            &CommandArgs::new()
                .with("key", "autosave.interval_secs")
                .with("value", "1"),
        );
        assert_eq!(
            rt.host.settings.get_f64("autosave.interval_secs", 0.0),
            5.0,
            "clamped to spec min"
        );
    }

    #[test]
    fn plugin_tier_gate_rules() {
        use api::LicenseTier;
        // No license service yet: satisfies() is the gate Phase 4 wires in.
        assert!(LicenseTier::Trial.satisfies(None));
        assert!(!LicenseTier::Trial.satisfies(Some(LicenseTier::Pro)));
        assert!(LicenseTier::OEM.satisfies(Some(LicenseTier::Pro)));
    }
}
