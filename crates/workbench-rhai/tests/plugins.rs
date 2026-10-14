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
