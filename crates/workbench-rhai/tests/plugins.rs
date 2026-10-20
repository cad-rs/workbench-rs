//! workbench-rhai 装载链测试:清单校验、发现、注册、命令执行与后台任务。

use std::path::PathBuf;

use workbench_api as wb;
use workbench_api::{CommandArgs, CommandResult};
use workbench_core::{AppRuntime, WorkbenchAppBuilder};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn demo_plugin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/rhai-demo")
}

fn build_with(dirs: Vec<PathBuf>) -> AppRuntime {
    WorkbenchAppBuilder::new("test.product", "测试产品")
        .with_plugin_stage(workbench_rhai::plugin_stage(dirs))
        .build()
        .expect("装配失败")
}

fn manifest_text(id: &str, api_version: &str) -> String {
    format!(
        "[plugin]\nid = \"{id}\"\nname = \"T\"\nversion = \"0.1.0\"\napi_version = \"{api_version}\"\nentry_point = \"plugin.rhai\"\n"
    )
}

#[test]
fn 清单_解析与版本检查() {
    let text = std::fs::read_to_string(fixture("good/plugin.toml")).unwrap();
    let manifest = workbench_rhai::parse_manifest(&text).unwrap();
    assert_eq!(manifest.plugin.id, "com.test.good");
    assert_eq!(manifest.plugin.api_version, "1");
    let (file, func) = workbench_rhai::split_entry_point(&manifest.plugin.entry_point);
    assert_eq!(file, "plugin.rhai");
    assert_eq!(func, "register");

    assert!(workbench_rhai::parse_manifest(&manifest_text("com.test.bad", "2")).is_err());
    assert!(workbench_rhai::parse_manifest(&manifest_text("", "1")).is_err());
    assert!(workbench_rhai::parse_manifest(&manifest_text("com.test.bad?", "1")).is_err());
}

#[test]
fn 装载_demo插件_注册命令工具栏与脚本面板() {
    let app = build_with(vec![demo_plugin().parent().unwrap().to_path_buf()]);
    assert!(
        app.plugins_loaded().contains(&"com.example.demo".to_string()),
        "loaded={:?} rejected={:?}",
        app.plugins_loaded(),
        app.host.plugins.iter().map(|p| (p.id.clone(), p.detail.clone())).collect::<Vec<_>>()
    );
    assert_eq!(app.command_title("rhai.hello"), Some("Rhai 问候".to_string()));
    assert_eq!(app.command_title("rhai.progress"), Some("Rhai 后台任务".to_string()));
    let toolbar = app
        .host
        .ws
        .ribbon
        .iter()
        .flat_map(|t| t.groups.iter())
        .find(|g| g.id.0 == "plugin.rhai")
        .expect("工具栏组");
    assert!(toolbar.items.len() >= 2);
    let panel = app
        .host
        .ws
        .panels
        .iter()
        .find(|p| p.id.0 == "rhai.panel")
        .expect("脚本面板");
    assert!(panel.script.as_deref().unwrap().ends_with("ui/main.rhai"));

    // A script central view registers the same way, but lands in the view table.
    let view = app
        .host
        .ws
        .view_def(&wb::ViewTypeId::new("rhai.view"))
        .expect("脚本视图");
    assert!(view.singleton, "脚本视图是单例");
    assert!(view.script.as_deref().unwrap().ends_with("ui/view.rhai"));
    assert!(
        app.host
            .ws
            .view_type_ids()
            .any(|t| t.as_str() == "rhai.view"),
        "脚本视图类型已注册"
    );
}

#[test]
fn 产品模块解析器收到插件声明的模块() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = seen.clone();
    let _app = WorkbenchAppBuilder::new("test.modules", "模块装配")
        // `plugin_stage` scans the *parent* directory for plugin subdirectories.
        .with_plugin_stage(workbench_rhai::plugin_stage(vec![fixture("with-modules")
            .parent()
            .unwrap()
            .to_path_buf()]))
        .with_module_loader(Box::new(move |_plugin, ids| {
            captured.lock().unwrap().extend(ids.iter().cloned());
            Vec::new()
        }))
        .build()
        .expect("装配失败");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &["wb.alpha".to_string(), "wb.beta".to_string()],
        "插件声明的模块 ID 应交给产品的模块解析器"
    );
}

/// A view whose content is nothing; the disable tests only need the type ids.
struct NoopView;

impl wb::ViewInstance for NoopView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("plugin.view")
    }
    fn title(&self) -> String {
        "插件视图".to_string()
    }
    fn element(
        &mut self,
        _host: &mut dyn wb::AppServices,
    ) -> Option<Box<dyn std::any::Any>> {
        None
    }
}

/// The workbench a plugin's module resolves to: it contributes one of each kind.
struct PluginWb;

impl wb::Workbench for PluginWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb.plugin")
    }
    fn init(&self, reg: &mut wb::Registry) {
        reg.register_command(wb::CommandDef::sync(
            "plugin.cmd",
            "插件命令",
            |_ctx, _args| wb::CommandResult::done(),
        ));
        reg.register_view(wb::ViewDef::new("plugin.view", "插件视图", true, || {
            Box::new(NoopView)
        }));
        reg.register_panel(wb::PanelDef::new(
            "plugin.panel",
            "插件面板",
            wb::DockArea::Right,
            300.0,
            9,
            || Box::new(NoopView),
        ));
    }
}

#[test]
fn 禁用插件移除其模块注册的贡献() {
    let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = served.clone();
    let mut app = WorkbenchAppBuilder::new("test.disable", "禁用")
        .with_plugin_stage(workbench_rhai::plugin_stage(vec![fixture("with-modules")
            .parent()
            .unwrap()
            .to_path_buf()]))
        // Only the first declared module resolves to a workbench; a second one
        // would register the same view type twice and be rejected.
        .with_module_loader(Box::new(move |_plugin, _ids| {
            // One workbench for the plugin: a second registration of the same
            // view type would be rejected as a duplicate.
            if served.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                vec![Box::new(PluginWb)]
            } else {
                Vec::new()
            }
        }))
        .build()
        .expect("装配失败");

    assert_eq!(app.command_title("plugin.cmd"), Some("插件命令".to_string()));
    assert!(app
        .host
        .ws
        .view_def(&wb::ViewTypeId::new("plugin.view"))
        .is_some());
    assert!(app.host.ws.panels.iter().any(|p| p.id.0 == "plugin.panel"));

    let removed = app.disable_plugin("com.test.modules").expect("禁用成功");
    assert!(!removed.is_empty(), "应移除模块注册的运动命令");

    assert_eq!(app.command_title("plugin.cmd"), None);
    assert!(app
        .host
        .ws
        .view_def(&wb::ViewTypeId::new("plugin.view"))
        .is_none());
    assert!(!app.host.ws.panels.iter().any(|p| p.id.0 == "plugin.panel"));
}

#[test]
fn 清单_解析模块声明并校验() {
    let text = std::fs::read_to_string(fixture("with-modules/plugin.toml")).unwrap();
    let manifest = workbench_rhai::parse_manifest(&text).unwrap();
    assert_eq!(manifest.plugin.modules, vec!["wb.alpha", "wb.beta"]);

    // A malformed module id is rejected with a readable reason.
    let bad = manifest_text("com.test.modules", "1").replace(
        "entry_point = \"plugin.rhai\"",
        "entry_point = \"plugin.rhai\"\nmodules = [\"wb.bad id\"]",
    );
    assert!(workbench_rhai::parse_manifest(&bad).is_err());
}

#[test]
fn 同步命令_脚本返回备注() {
    let mut app = build_with(vec![demo_plugin().parent().unwrap().to_path_buf()]);
    let result = app.execute_command("rhai.hello", &CommandArgs::new().with("name", "自检"));
    match result {
        CommandResult::Done { note } => assert!(
            note.as_ref().map(|n| n.contains("问候")).unwrap_or(false),
            "note={note:?}"
        ),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn 后台命令_进度完成() {
    let mut app = build_with(vec![demo_plugin().parent().unwrap().to_path_buf()]);
    let _ = app.execute_command("rhai.progress", &CommandArgs::default());
    for _ in 0..300 {
        app.frame_tick(0.016);
        if app.host.tasks.summary().0 == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app
        .host
        .tasks
        .all()
        .any(|t| t.title.contains("Rhai") && t.status == wb::TaskStatus::Succeeded));
}

#[test]
fn 插件_被拒绝的脚本不致崩溃() {
    let bad_dir = fixture("bad-syntax");
    let app = build_with(vec![bad_dir]);
    assert!(app.plugins_loaded().is_empty());
    assert_eq!(app.command_title("rhai.hello"), None);
}
