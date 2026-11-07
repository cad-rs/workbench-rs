//! workbench-rhai loading-chain tests: manifest validation, discovery, registration, command
//! execution and background tasks.

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
    WorkbenchAppBuilder::new("test.product", "Test Product")
        .with_plugin_stage(workbench_rhai::plugin_stage(dirs))
        .build()
        .expect("assembly failed")
}

fn manifest_text(id: &str, api_version: &str) -> String {
    format!(
        "[plugin]\nid = \"{id}\"\nname = \"T\"\nversion = \"0.1.0\"\napi_version = \"{api_version}\"\nentry_point = \"plugin.rhai\"\n"
    )
}

#[test]
fn manifest_parse_and_version_check() {
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
fn loading_demo_plugin_registers_commands_toolbar_and_script_panel() {
    let app = build_with(vec![demo_plugin().parent().unwrap().to_path_buf()]);
    assert!(
        app.plugins_loaded().contains(&"com.example.demo".to_string()),
        "loaded={:?} rejected={:?}",
        app.plugins_loaded(),
        app.host.plugins.iter().map(|p| (p.id.clone(), p.detail.clone())).collect::<Vec<_>>()
    );
    assert_eq!(app.command_title("rhai.hello"), Some("Rhai Hello".to_string()));
    assert_eq!(app.command_title("rhai.progress"), Some("Rhai Background Task".to_string()));
    let toolbar = app
        .host
        .ws
        .ribbon
        .iter()
        .flat_map(|t| t.groups.iter())
        .find(|g| g.id.0 == "plugin.rhai")
        .expect("toolbar group");
    assert!(toolbar.items.len() >= 2);
    let panel = app
        .host
        .ws
        .panels
        .iter()
        .find(|p| p.id.0 == "rhai.panel")
        .expect("script panel");
    assert!(panel.script.as_deref().unwrap().ends_with("ui/main.rhai"));

    // A script central view registers the same way, but lands in the view table.
    let view = app
        .host
        .ws
        .view_def(&wb::ViewTypeId::new("rhai.view"))
        .expect("script view");
    assert!(view.singleton, "the script view is a singleton");
    assert!(view.script.as_deref().unwrap().ends_with("ui/view.rhai"));
    assert!(
        app.host
            .ws
            .view_type_ids()
            .any(|t| t.as_str() == "rhai.view"),
        "the script view type is registered"
    );
}

#[test]
fn product_module_loader_receives_plugin_declared_modules() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = seen.clone();
    let _app = WorkbenchAppBuilder::new("test.modules", "Module Assembly")
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
        .expect("assembly failed");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &["wb.alpha".to_string(), "wb.beta".to_string()],
        "the module IDs a plugin declares should reach the product's module loader"
    );
}

/// A view whose content is nothing; the disable tests only need the type ids.
struct NoopView;

impl wb::ViewInstance for NoopView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("plugin.view")
    }
    fn title(&self) -> String {
        "Plugin View".to_string()
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
            "Plugin Command",
            |_ctx, _args| wb::CommandResult::done(),
        ));
        reg.register_view(wb::ViewDef::new("plugin.view", "Plugin View", true, || {
            Box::new(NoopView)
        }));
        reg.register_panel(wb::PanelDef::new(
            "plugin.panel",
            "Plugin Panel",
            wb::DockArea::Right,
            300.0,
            9,
            || Box::new(NoopView),
        ));
    }
}

#[test]
fn disabling_a_plugin_removes_its_module_contributions() {
    let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = served.clone();
    let mut app = WorkbenchAppBuilder::new("test.disable", "Disable")
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
        .expect("assembly failed");

    assert_eq!(app.command_title("plugin.cmd"), Some("Plugin Command".to_string()));
    assert!(app
        .host
        .ws
        .view_def(&wb::ViewTypeId::new("plugin.view"))
        .is_some());
    assert!(app.host.ws.panels.iter().any(|p| p.id.0 == "plugin.panel"));

    let removed = app.disable_plugin("com.test.modules").expect("disable succeeds");
    assert!(!removed.is_empty(), "the commands the module registered should be removed");

    assert_eq!(app.command_title("plugin.cmd"), None);
    assert!(app
        .host
        .ws
        .view_def(&wb::ViewTypeId::new("plugin.view"))
        .is_none());
    assert!(!app.host.ws.panels.iter().any(|p| p.id.0 == "plugin.panel"));
}

#[test]
fn manifest_parses_and_validates_module_declarations() {
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
fn sync_command_script_returns_note() {
    let mut app = build_with(vec![demo_plugin().parent().unwrap().to_path_buf()]);
    let result = app.execute_command("rhai.hello", &CommandArgs::new().with("name", "selfcheck"));
    match result {
        CommandResult::Done { note } => assert!(
            note.as_ref().map(|n| n.contains("Hello")).unwrap_or(false),
            "note={note:?}"
        ),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn background_command_progress_completes() {
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
fn rejected_plugin_script_does_not_crash() {
    let bad_dir = fixture("bad-syntax");
    let app = build_with(vec![bad_dir]);
    assert!(app.plugins_loaded().is_empty());
    assert_eq!(app.command_title("rhai.hello"), None);
}
