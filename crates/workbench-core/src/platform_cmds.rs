//! Platform built-in commands (design.md §12.1 document lifecycle + §12.2 undo/redo).
//!
//! Commands operate the platform through the AppServices facade and never touch the GUI;
//! hotkey bindings are registered here in one place.

use workbench_api as api;
use api::{
    CommandArgs, CommandCtx, CommandDef, CommandResult, Hotkey, KeyCode, Registry, SettingKind,
    SettingScope, SettingSpec,
};

pub const NEW_DOCUMENT: &str = "app.new_document";
pub const OPEN_DOCUMENT: &str = "app.open_document";
pub const SAVE_DOCUMENT: &str = "app.save_document";
pub const SAVE_DOCUMENT_AS: &str = "app.save_document_as";
pub const IMPORT_DOCUMENT: &str = "app.import_document";
pub const EXPORT_DOCUMENT: &str = "app.export_document";
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

/// The built-in Chinese UI (`ui.locale = "zh-CN"`). The English strings at
/// the call sites are the keys (Qt tr() style): display sites call
/// `host.tr(key)`, and a key without an entry renders as written.
fn register_builtin_locales(registry: &mut Registry) {
    registry.register_locales(
        "zh-CN",
        [
            // Document lifecycle commands.
            ("New Document", "新建文档"),
            ("Open Document…", "打开文档…"),
            ("Open Document", "打开文档"),
            ("Save Document", "保存文档"),
            ("Save", "保存"),
            ("Save As…", "另存为…"),
            ("Import…", "导入…"),
            ("Export…", "导出…"),
            ("Undo", "撤销"),
            ("Redo", "重做"),
            ("Close Tab", "关闭当前标签"),
            ("Collapse/Expand Ribbon", "折叠/展开 Ribbon"),
            // Plugin & settings commands.
            ("Disable Plugin", "禁用插件"),
            ("Autosave Now", "立即自动保存"),
            ("Change Setting", "修改设置"),
            ("About", "关于"),
            ("Show/Hide ", "显示/隐藏 "),
            // Ribbon groups & tabs.
            ("View", "窗口"),
            ("Help", "帮助"),
            ("Home", "主页"),
            ("General", "常规"),
            ("Appearance", "外观"),
            // Panel titles.
            ("Diagnostics", "诊断"),
            ("Plugins", "插件"),
            ("Settings", "设置"),
            ("Welcome", "欢迎"),
            // Settings panel rows.
            ("Language", "语言"),
            ("Theme", "主题"),
            ("Dark", "深色"),
            ("Light", "浅色"),
            ("Accent color", "强调色"),
            ("Autosave interval (s)", "自动保存间隔（秒）"),
            // Settings values / controls.
            ("On", "开"),
            ("Off", "关"),
            // Diagnostics panel.
            ("Runtime", "运行时"),
            ("Product", "产品"),
            ("(see window title)", "（见窗口标题）"),
            ("GUI backend", "GUI 后端"),
            ("(unlabeled)", "（未标注）"),
            ("Uptime", "运行时长"),
            ("s", "秒"),
            ("Frame stats", "帧统计"),
            ("ms/frame (recent ", "ms/帧（最近 "),
            ("Documents & tasks", "文档与任务"),
            ("Documents", "文档数"),
            ("Unsaved", "未保存"),
            ("Background tasks", "后台任务"),
            ("running ", "运行 "),
            (" / total ", " / 累计 "),
            ("Autosave", "自动保存"),
            ("Interval", "间隔"),
            ("s (tunable in settings.json)", "秒（settings.json 可调）"),
            ("Autosaved", "已自动保存"),
            // Plugins panel.
            ("No plugins registered", "没有已注册的插件"),
            ("Loaded", "已装载"),
            ("Disabled", "已禁用"),
            ("Rejected", "已拒绝"),
            ("Disable", "禁用"),
            // About dialog.
            ("Version", "版本"),
            ("Support", "支持"),
            ("Close", "关闭"),
            // Window-state log lines.
            ("Collapsed", "已折叠"),
            ("Expanded", "已展开"),
            // Status bar.
            ("Doc: ", "文档："),
            ("(none)", "（无）"),
            (" · rev r", " · 修订 r"),
            ("Mode: ", "模式："),
            ("Cancel", "取消"),
            // Account chip.
            ("Sign in", "登录"),
            ("Sign out", "登出"),
            // File picker.
            ("Open", "打开"),
            ("Overwrite", "覆盖"),
            ("New Folder", "新建文件夹"),
            ("Browser storage", "浏览器存储"),
            ("Local files", "本地文件"),
            ("Click a file to open it", "单击文件打开"),
            ("Loading…", "正在读取…"),
            ("(empty)", "（空）"),
            // Other frontend text.
            ("Edit Setting", "编辑设置"),
            ("No open views", "没有打开的视图"),
            ("Enter to save · Esc to cancel", "Enter 保存 · Esc 取消"),
        ],
    );
}

/// Built-in setting specs: the settings panel renders rows from these and
/// `app.settings.set` validates writes against them.
fn register_builtin_settings(registry: &mut Registry) {
    // English is the source language; products can default to another one
    // through product.toml's `[i18n] default_locale`.
    registry.register_setting(SettingSpec {
        key: "ui.locale".into(),
        label: "Language".into(),
        group: "Appearance".into(),
        kind: SettingKind::Enum(vec!["en".into(), "zh-CN".into()]),
        default: serde_json::json!("en"),
        scope: SettingScope::User,
    });
    registry.register_setting(SettingSpec {
        key: "ui.theme".into(),
        label: "Theme".into(),
        group: "Appearance".into(),
        kind: SettingKind::Enum(vec!["dark".into(), "light".into()]),
        default: serde_json::json!("dark"),
        scope: SettingScope::User,
    });
    // Brand accent (`#RRGGBB`), seeded from product.toml's branding.accent at
    // build time when the product ships one.
    registry.register_setting(SettingSpec {
        key: "ui.accent".into(),
        label: "Accent color".into(),
        group: "Appearance".into(),
        kind: SettingKind::Str,
        default: serde_json::json!("#338CF2"),
        scope: SettingScope::User,
    });
    registry.register_setting(SettingSpec {
        key: "autosave.interval_secs".into(),
        label: "Autosave interval (s)".into(),
        group: "General".into(),
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

    // ---- Platform built-in settings ----
    register_builtin_settings(registry);

    // ---- Platform built-in translations (zh-CN) ----
    register_builtin_locales(registry);

    // ---- Plugin management commands ----
    registry.register_command(
        CommandDef::sync(
            "app.plugins.disable",
            "Disable Plugin",
            |ctx: &mut CommandCtx, args: &CommandArgs| {
                let Some(id) = args.get("id") else {
                    return CommandResult::failed("Missing argument: id");
                };
                match ctx.disable_plugin(id) {
                    Ok(removed) => CommandResult::done_with(format!(
                        "Disabled {id} ({} commands removed)",
                        removed.len()
                    )),
                    Err(e) => CommandResult::failed(e),
                }
            },
        )
        .enabled_when(|_| true),
    );

    // ---- Autosave command ----
    registry.register_command(
        CommandDef::sync(
            "app.autosave.now",
            "Autosave Now",
            |ctx: &mut CommandCtx, _args: &CommandArgs| match ctx.autosave_now() {
                Ok(n) => CommandResult::done_with(format!("Autosaved {n} documents")),
                Err(e) => CommandResult::failed(e),
            },
        ),
    );

    // ---- Document lifecycle commands ----    // New document
    registry.register_command(
        CommandDef::sync(NEW_DOCUMENT, "New Document", |ctx: &mut CommandCtx, args: &CommandArgs| {
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
                return CommandResult::failed("The product registers no document types");
            };
            let count = ctx.host().docs.docs().count();
            let title = if count == 0 {
                "Untitled".to_string()
            } else {
                format!("Untitled {}", count + 1)
            };
            let doc_id = match ctx.host().docs.create(&type_id, &title) {
                Ok(id) => id,
                Err(e) => return CommandResult::failed(e),
            };
            ctx.host().docs.set_active(Some(doc_id));
            if let Some(vt) = preferred_view_of(ctx, doc_id) {
                if let Err(e) = ctx.open_view(vt.as_str(), true) {
                    ctx.host().log.warn(format!("Failed to open the view: {e}"));
                }
            }
            ctx.host().log.info(format!("Created document \"{title}\""));
            CommandResult::done()
        })
        .icon(crate::assets::icons::NEW)
        .hotkey(Hotkey::new(KeyCode::N).ctrl(true)),
    );

    // Open document
    registry.register_command(
        CommandDef::sync(OPEN_DOCUMENT, "Open Document…", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            request_open_dialog(ctx);
            CommandResult::done_with("Opened the file dialog")
        })
        .icon(crate::assets::icons::OPEN)
        .hotkey(Hotkey::new(KeyCode::O).ctrl(true)),
    );

    // Save
    registry.register_command(
        CommandDef::sync(SAVE_DOCUMENT, "Save", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("No active document");
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

    // Save as
    registry.register_command(
        CommandDef::sync(SAVE_DOCUMENT_AS, "Save As…", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("No active document");
            };
            request_save_as_dialog(ctx, doc_id)
        })
        .hotkey(Hotkey::new(KeyCode::S).ctrl(true).shift(true))
        .enabled_when(|snap| snap.active_document.is_some()),
    );

    // Import / export: move a copy to or from the user's device (the browser's file chooser
    // and downloads on the web)
    registry.register_command(CommandDef::sync(
        IMPORT_DOCUMENT,
        "Import…",
        |ctx: &mut CommandCtx, _args: &CommandArgs| {
            request_import(ctx);
            CommandResult::done()
        },
    ));
    registry.register_command(
        CommandDef::sync(EXPORT_DOCUMENT, "Export…", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("No active document");
            };
            request_export(ctx, doc_id)
        })
        .enabled_when(|snap| snap.active_document.is_some()),
    );
    crate::file_picker::register(registry);

    // Undo / redo
    registry.register_command(
        CommandDef::sync(UNDO, "Undo", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("No active document");
            };
            match ctx.host().docs.undo(doc_id) {
                Ok(label) => {
                    ctx.host().log.info(format!("Undid \"{label}\""));
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
        CommandDef::sync(REDO, "Redo", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            let Some(doc_id) = ctx.active_document() else {
                return CommandResult::failed("No active document");
            };
            match ctx.host().docs.redo(doc_id) {
                Ok(label) => {
                    ctx.host().log.info(format!("Redid \"{label}\""));
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

    // Close the active center tab
    registry.register_command(
        CommandDef::sync(CLOSE_ACTIVE_VIEW, "Close Tab", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            match ctx.close_active_view() {
                Ok(_) => CommandResult::done(),
                Err(e) => CommandResult::failed(e),
            }
        })
        .hotkey(Hotkey::new(KeyCode::W).ctrl(true)),
    );

    // Ribbon collapse toggle (Ctrl+F1, only when enabled, ribbon-bar.md §4.5)
    registry.register_command(
        CommandDef::sync(
            "app.toggle_ribbon_collapse",
            "Collapse/Expand Ribbon",
            |ctx: &mut CommandCtx, _args: &CommandArgs| {
                let host = ctx.host();
                if !host.ws.ribbon_collapse_enabled {
                    return CommandResult::failed("Ribbon collapsing is not enabled");
                }
                host.ws.ribbon_collapsed = !host.ws.ribbon_collapsed;
                CommandResult::done_with(if host.ws.ribbon_collapsed {
                    "Collapsed"
                } else {
                    "Expanded"
                })
            },
        )
        .hotkey(Hotkey::new(KeyCode::F1).ctrl(true)),
    );

    // Settings writes: type-checked and range-clamped against the registered spec, written and
    // persisted at once. CommandArgs is a string table; the string is parsed into a Value by
    // the spec's type, then validated.
    registry.register_command(
        CommandDef::sync(
            SETTINGS_SET,
            "Change Setting",
            |ctx: &mut CommandCtx, args: &CommandArgs| {
                let Some(key) = args.get("key") else {
                    return CommandResult::failed("Missing argument: key");
                };
                let Some(raw) = args.get("value") else {
                    return CommandResult::failed("Missing argument: value");
                };
                let host = ctx.host();
                let Some(spec) = host.setting_specs.iter().find(|s| s.key == key).cloned() else {
                    return CommandResult::failed(format!("Unregistered setting: {key}"));
                };
                let parsed = match &spec.kind {
                    SettingKind::Bool => match raw {
                        "true" => serde_json::Value::Bool(true),
                        "false" => serde_json::Value::Bool(false),
                        _ => return CommandResult::failed(format!("{raw} is not a boolean")),
                    },
                    SettingKind::Int { .. } => match raw.parse::<i64>() {
                        Ok(n) => serde_json::Value::Number(n.into()),
                        Err(_) => return CommandResult::failed(format!("{raw} is not an integer")),
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
                        host.log.info(format!("[Settings] {} = {normalized}", spec.key));
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
            "Edit Setting",
            |ctx: &mut CommandCtx, args: &CommandArgs| {
                let Some(key) = args.get("key") else {
                    return CommandResult::failed("Missing argument: key");
                };
                let host = ctx.host();
                let Some(spec) = host.setting_specs.iter().find(|s| s.key == key).cloned() else {
                    return CommandResult::failed(format!("Unregistered setting: {key}"));
                };
                // Free-form strings are the only editable-by-hand kind for
                // now (bool/enum/int have dedicated controls).
                if !matches!(spec.kind, SettingKind::Str) {
                    return CommandResult::failed(format!("{} is not a text setting", spec.key));
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

    // The About dialog: sets a workspace flag; the frontend (workbench-ui) renders the dialog.
    registry.register_command(
        CommandDef::sync(ABOUT, "About", |ctx: &mut CommandCtx, _args: &CommandArgs| {
            ctx.host().ws.about_open = true;
            CommandResult::done()
        })
        .enabled_when(|_| true),
    );

    // A show/hide command per panel + the "View" toolbar group (including the platform's own
    // panels)
    let all_panels: Vec<(String, String)> = panel_ids.to_vec();
    let mut window_group = api::ToolbarGroup::new("app.window", "View");
    for (id, title) in &all_panels {
        let panel_id = id.clone();
        let pid = panel_id.clone();
        let title = title.clone();
        registry.register_command(
            CommandDef::sync(
                format!("{TOGGLE_PANEL_PREFIX}{id}"),
                format!("Show/Hide {title}"),
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

    // The "Help" toolbar group: the About dialog entry (product branding info).
    let mut help_group = api::ToolbarGroup::new("app.help", "Help");
    help_group
        .items
        .push(api::ToolbarItem::labeled(ABOUT, "About"));
    registry.add_toolbar_group(help_group);
}

/// Open the document at `location`: its storage provider reads the file off
/// the UI thread, then the document is decoded, made active and shown in its
/// preferred view. Failures go to the log.
pub fn open_document(ctx: &mut CommandCtx, location: api::Location) {
    let type_id = match ctx.host().docs.type_for(location.file_name()) {
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
        show_opened(&mut ctx, opened, format!("Opened {location}"));
    }));
}

/// Add the document in `bytes` from a file called `name` that it does not
/// stay tied to (an import): its first save asks where to put it.
fn import_document(ctx: &mut CommandCtx, name: &str, bytes: &[u8]) {
    let opened = ctx.host().docs.type_for(name).and_then(|type_id| {
        let title = api::storage::split_file_name(name).0;
        ctx.host().docs.open_bytes(&type_id, title, None, bytes)
    });
    show_opened(ctx, opened, format!("Imported {name}"));
}

/// Make a just-opened document active and show it in its preferred view; a
/// failure goes to the log.
fn show_opened(ctx: &mut CommandCtx, opened: Result<api::DocumentId, String>, done: String) {
    let doc_id = match opened {
        Ok(id) => id,
        Err(e) => {
            ctx.host().log.error(e);
            return;
        }
    };
    ctx.host().docs.set_active(Some(doc_id));
    ctx.host().log.info(done);
    if let Some(vt) = preferred_view_of(ctx, doc_id) {
        if let Err(e) = ctx.open_view(vt.as_str(), true) {
            ctx.host().log.warn(format!("Failed to open the view: {e}"));
        }
    }
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
            Ok(location) if save_as => host.log.info(format!("Saved as {location}")),
            Ok(location) => host.log.info(format!("Saved to {location}")),
            Err(e) => host.log.error(e),
        }
    }));
    CommandResult::done()
}

/// Ask the platform to show the "Open Document" dialog on a background thread, and open the
/// pick on the UI thread.
///
/// A blocking OS modal loop must never run on the UI thread (it re-enters gpui's window
/// procedure and wedges the render loop), hence [`CommandCtx::request_file_dialog`].
fn request_open_dialog(ctx: &mut CommandCtx) {
    let exts = all_extensions(ctx);
    ctx.request_file_dialog(exts, None, Box::new(|ctx, picked| {
        if let Some(location) = picked {
            open_document(&mut CommandCtx::new(ctx), location);
        }
    }));
}

/// A document's file name (its title, with its type's extension unless the
/// title has one) and its type's extensions.
fn file_name_of(ctx: &mut CommandCtx, doc_id: api::DocumentId) -> Option<(String, Vec<String>)> {
    let host = ctx.host();
    let doc = host.docs.get(doc_id)?;
    let exts = host
        .docs
        .type_of(&doc.type_id)
        .map(|t| t.extensions.clone())
        .unwrap_or_default();
    let name = if doc.title.contains('.') {
        doc.title.clone()
    } else {
        format!("{}.{}", doc.title, exts.first().map(|s| s.as_str()).unwrap_or("dat"))
    };
    Some((name, exts))
}

/// Ask the platform to show the "Save As" dialog on a background thread, and write the file
/// from the UI thread once picked.
fn request_save_as_dialog(ctx: &mut CommandCtx, doc_id: api::DocumentId) -> CommandResult {
    let Some((default_name, exts)) = file_name_of(ctx, doc_id) else {
        return CommandResult::failed("No such document");
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
    CommandResult::done_with("Opened the file dialog")
}

/// Import: bring in a copy of a file from the user's device (the browser's
/// file chooser on the web, the file dialog on the desktop). The document is
/// not tied to the file.
fn request_import(ctx: &mut CommandCtx) {
    let exts = all_extensions(ctx);
    #[cfg(target_arch = "wasm32")]
    ctx.spawn_io(Box::pin(async move {
        let picked = crate::web_files::pick(&exts).await;
        Box::new(move |app: &mut dyn api::AppServices| {
            let mut ctx = CommandCtx::new(app);
            match picked {
                Ok((name, bytes)) => import_document(&mut ctx, &name, &bytes),
                Err(e) => ctx.host().log.info(e),
            }
        }) as api::UiThen
    }));
    #[cfg(not(target_arch = "wasm32"))]
    ctx.request_file_dialog(exts, None, Box::new(|app, picked| {
        let Some(location) = picked else {
            return;
        };
        CommandCtx::new(app).read_file(location.clone(), Box::new(move |app, bytes| {
            let mut ctx = CommandCtx::new(app);
            match bytes {
                Ok(bytes) => import_document(&mut ctx, location.file_name(), &bytes),
                Err(e) => ctx.host().log.error(e),
            }
        }));
    }));
}

/// Export: hand a copy of a document to the user's device (a download on the
/// web, the file dialog on the desktop). The document stays where it is.
fn request_export(ctx: &mut CommandCtx, doc_id: api::DocumentId) -> CommandResult {
    let Some((name, exts)) = file_name_of(ctx, doc_id) else {
        return CommandResult::failed("No such document");
    };
    #[cfg(target_arch = "wasm32")]
    {
        let _ = exts;
        let exported = ctx
            .host()
            .docs
            .encode(doc_id)
            .and_then(|bytes| crate::web_files::download(&name, &bytes));
        match exported {
            Ok(()) => CommandResult::done_with(format!("Exported {name}")),
            Err(e) => CommandResult::failed(e),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        ctx.request_file_dialog(exts, Some(name), Box::new(move |app, picked| {
            let Some(location) = picked else {
                return;
            };
            let mut ctx = CommandCtx::new(app);
            let bytes = match ctx.host().docs.encode(doc_id) {
                Ok(bytes) => bytes,
                Err(e) => return ctx.host().log.error(e),
            };
            ctx.write_file(location.clone(), bytes, Box::new(move |app, written| {
                match written {
                    Ok(()) => app.host().log.info(format!("Exported to {location}")),
                    Err(e) => app.host().log.error(e),
                }
            }));
        }));
        CommandResult::done_with("Opened the file dialog")
    }
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
