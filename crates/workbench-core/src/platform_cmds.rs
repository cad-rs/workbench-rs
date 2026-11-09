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
pub const EXPORT_DIAGNOSTICS: &str = "app.export_diagnostics";
pub const OPEN_SETTINGS: &str = "app.settings.open";
pub const OPEN_SHORTCUTS: &str = "app.shortcuts.open";
pub const REFRESH_LICENSE: &str = "app.license.refresh";
pub const TOGGLE_RIBBON_COLLAPSE: &str = "app.toggle_ribbon_collapse";
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
            ("Export Diagnostics…", "导出诊断信息…"),
            ("Send crash reports", "发送崩溃报告"),
            ("Undo", "撤销"),
            ("Redo", "重做"),
            ("Close Tab", "关闭当前标签"),
            ("Collapse/Expand Ribbon", "折叠/展开 Ribbon"),
            // Plugin & settings commands.
            ("Disable Plugin", "禁用插件"),
            ("Autosave Now", "立即自动保存"),
            ("Change Setting", "修改设置"),
            ("About", "关于"),
            ("Commands & Shortcuts", "快捷键命令表"),
            ("Refresh License", "刷新许可"),
            ("Other", "其他"),
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
            // Settings dialog rows.
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

/// Built-in setting specs: the settings dialog renders rows from these and
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

/// Register the platform commands. A panel's toggle command inherits its icon,
/// so the Home tab's "View" group shows the same glyph the panel header does,
/// and the feature it requires, so the toggle is disabled with the panel.
pub fn register(registry: &mut Registry, ribbon_collapse: bool) {
    // The built-in platform panels (diagnostics / plugins) are view
    // implementations, so the frontend registers them before this runs; their
    // ids therefore already appear in `registry.panels` below.

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
        ),
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

    // Ribbon collapse toggle (Ctrl+F1, ribbon-bar.md §4.5)
    if ribbon_collapse {
        registry.register_command(
            CommandDef::sync(
                TOGGLE_RIBBON_COLLAPSE,
                "Collapse/Expand Ribbon",
                |ctx: &mut CommandCtx, _args: &CommandArgs| {
                    let host = ctx.host();
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
    }

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
        ),
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
        ),
    );

    use crate::assets::icons;
    registry.register_command(dialog_command(ABOUT, "About", icons::ABOUT, api::PlatformDialog::About));
    registry.register_command(dialog_command(
        OPEN_SETTINGS,
        "Settings",
        icons::SETTINGS,
        api::PlatformDialog::Settings,
    ));
    registry.register_command(dialog_command(
        OPEN_SHORTCUTS,
        "Commands & Shortcuts",
        icons::KEYBOARD,
        api::PlatformDialog::Shortcuts,
    ));

    // Refresh License: only for a product that limits features.
    let limits_features = registry.entitlements.is_some();
    if limits_features {
        registry.register_command(
            CommandDef::sync(
                REFRESH_LICENSE,
                "Refresh License",
                |ctx: &mut CommandCtx, _args: &CommandArgs| {
                    ctx.refresh_entitlements();
                    CommandResult::done()
                },
            )
            .icon(crate::assets::icons::REFRESH),
        );
    }

    registry.register_command(
        CommandDef::sync(
            EXPORT_DIAGNOSTICS,
            "Export Diagnostics…",
            |ctx: &mut CommandCtx, _args: &CommandArgs| {
                save_copy(ctx, "diagnostics.txt".to_string(), vec!["txt".to_string()], |ctx| {
                    Ok(ctx.diagnostics_report().into_bytes())
                })
            },
        )
        .icon(crate::assets::icons::DIAGNOSTICS),
    );

    // A show/hide command per panel + the "View" toolbar group (including the platform's own
    // panels)
    let panels: Vec<_> = registry
        .panels
        .iter()
        .map(|p| (p.id.0.clone(), p.title.clone(), p.icon.clone(), p.requires.clone()))
        .collect();
    let mut window_group = api::ToolbarGroup::new("app.window", "View");
    for (id, title, icon, requires) in panels {
        let command = format!("{TOGGLE_PANEL_PREFIX}{id}");
        let mut def = CommandDef::sync(
            command.clone(),
            format!("Show/Hide {title}"),
            move |ctx: &mut CommandCtx, _args: &CommandArgs| match ctx.toggle_panel(&id) {
                Ok(_) => CommandResult::done(),
                Err(e) => CommandResult::failed(e),
            },
        );
        // The panel's own icon (wb-std/* for the platform panels, the owning
        // workbench's path for theirs) — the ribbon toggle shows it too.
        if let Some(icon) = icon {
            def = def.icon(icon);
        }
        if let Some(feature) = requires {
            def = def.requires(feature);
        }
        registry.register_command(def);
        window_group.items.push(api::ToolbarItem::labeled(command, title));
    }
    window_group.items.push(api::ToolbarItem::command(OPEN_SETTINGS));
    registry.add_toolbar_group(window_group);

    // The "Help" toolbar group: the About dialog (product branding info) and what support needs.
    let mut help_group = api::ToolbarGroup::new("app.help", "Help");
    help_group
        .items
        .extend([OPEN_SHORTCUTS, ABOUT, EXPORT_DIAGNOSTICS].map(api::ToolbarItem::command));
    if limits_features {
        help_group.items.push(api::ToolbarItem::command(REFRESH_LICENSE));
    }
    registry.add_toolbar_group(help_group);
}

/// A command that opens a platform dialog, which the frontend renders.
fn dialog_command(id: &str, title: &str, icon: &str, dialog: api::PlatformDialog) -> CommandDef {
    CommandDef::sync(id, title, move |ctx: &mut CommandCtx, _args: &CommandArgs| {
        ctx.host().ws.dialog = Some(dialog);
        CommandResult::done()
    })
    .icon(icon)
}

/// Open the document at `location`: its storage provider reads the file off
/// the UI thread, then the document is decoded, made active and shown in its
/// preferred view. Failures go to the log.
pub fn open_document(ctx: &mut CommandCtx, location: api::Location) {
    read_document(ctx, location, true);
}

/// Read the document at `location` off the UI thread and add it: tied to
/// that file, or as an untied copy (`keep_file == false`: an import). A file
/// of an unknown type is refused before it is read.
fn read_document(ctx: &mut CommandCtx, location: api::Location, keep_file: bool) {
    if let Err(e) = ctx.host().docs.type_for(location.file_name()) {
        return ctx.host().log.error(e);
    }
    ctx.read_file(location.clone(), Box::new(move |app, bytes| {
        let name = location.file_name().to_string();
        let (location, done) = match keep_file {
            true => (Some(location.clone()), format!("Opened {location}")),
            false => (None, format!("Imported {name}")),
        };
        add_document(&mut CommandCtx::new(app), &name, location, bytes, done);
    }));
}

/// Decode `bytes` from a file called `name` into a new document — tied to
/// `location`, or untied when None so its first save asks where to put it —
/// make it active and show it in its preferred view. Failures go to the log.
fn add_document(
    ctx: &mut CommandCtx,
    name: &str,
    location: Option<api::Location>,
    bytes: Result<Vec<u8>, String>,
    done: String,
) {
    let opened = bytes.and_then(|bytes| {
        let type_id = ctx.host().docs.type_for(name)?;
        let title = api::storage::split_file_name(name).0;
        ctx.host().docs.open_bytes(&type_id, title, location, &bytes)
    });
    let doc_id = match opened {
        Ok(id) => id,
        Err(e) => {
            ctx.host().log.error(e);
            return;
        }
    };
    ctx.host().docs.set_active(Some(doc_id));
    ctx.host().log.info(done);
    if let Some(from) = ctx.host().docs.get(doc_id).and_then(|doc| doc.upgraded_from) {
        ctx.host().log.info(format!("Upgraded {name} from file format {from}"));
    }
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
    let kept = save.keep_old_at.clone();
    ctx.write_save(save, bytes, Box::new(move |app, saved| {
        let host = app.host();
        match saved {
            Ok(location) => {
                let saved = if save_as { "Saved as" } else { "Saved to" };
                host.log.info(format!("{saved} {location}"));
                if let Some(kept) = kept {
                    host.log.info(format!("The file's older format is kept as {kept}"));
                }
            }
            Err(e) => host.log.error(e),
        }
    }));
    CommandResult::done()
}

/// Ask the user for a document to open, then open it.
fn request_open_dialog(ctx: &mut CommandCtx) {
    let exts = all_extensions(ctx);
    ctx.request_file_dialog(exts, None, Box::new(|ctx, picked| {
        if let Some(location) = picked {
            open_document(&mut CommandCtx::new(ctx), location);
        }
    }));
}

/// A document's file name (its title with one of its type's extensions, the
/// rule the in-app picker applies too) and its type's extensions.
fn file_name_of(ctx: &mut CommandCtx, doc_id: api::DocumentId) -> Option<(String, Vec<String>)> {
    let host = ctx.host();
    let doc = host.docs.get(doc_id)?;
    let exts = host
        .docs
        .type_of(&doc.type_id)
        .map(|t| t.extensions.clone())
        .unwrap_or_default();
    Some((api::storage::with_extension(&doc.title, &exts), exts))
}

/// Ask the user where to save a document, then write it there.
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
                Ok((name, bytes)) => {
                    let done = format!("Imported {name}");
                    add_document(&mut ctx, &name, None, Ok(bytes), done);
                }
                Err(e) => ctx.host().log.info(e),
            }
        }) as api::UiThen
    }));
    #[cfg(not(target_arch = "wasm32"))]
    ctx.request_file_dialog(exts, None, Box::new(|app, picked| {
        if let Some(location) = picked {
            read_document(&mut CommandCtx::new(app), location, false);
        }
    }));
}

/// Export: hand a copy of a document to the user's device (a download on the
/// web, the file dialog on the desktop). The document stays where it is.
fn request_export(ctx: &mut CommandCtx, doc_id: api::DocumentId) -> CommandResult {
    let Some((name, exts)) = file_name_of(ctx, doc_id) else {
        return CommandResult::failed("No such document");
    };
    save_copy(ctx, name, exts, move |ctx| ctx.host().docs.encode(doc_id))
}

/// Hand a copy of some bytes to the user's device as `name`: a download on
/// the web, the file dialog on the desktop. `bytes` makes them once they are
/// wanted, so a cancelled dialog costs nothing.
fn save_copy(
    ctx: &mut CommandCtx,
    name: String,
    exts: Vec<String>,
    bytes: impl FnOnce(&mut CommandCtx) -> Result<Vec<u8>, String> + 'static,
) -> CommandResult {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = exts;
        match bytes(ctx).and_then(|bytes| crate::web_files::download(&name, &bytes)) {
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
            let bytes = match bytes(&mut ctx) {
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
}
