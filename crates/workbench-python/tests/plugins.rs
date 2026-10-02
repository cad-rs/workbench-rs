//! workbench-python 集成测试：清单校验、发现加载、命令执行与错误隔离（design.md §11、验收 #12/#19/#20）。

use std::path::PathBuf;

use workbench_api as wb;
use workbench_core::{AppRuntime, WorkbenchAppBuilder};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

// ============ 清单（纯 Rust，不需要解释器） ============

#[test]
fn 清单_解析与版本检查() {
    let m = workbench_python::parse_manifest(&fixture("good-plugin").join("plugin.toml"))
        .expect("good-plugin 清单应可解析");
    assert_eq!(m.plugin.id, "test.good");
    assert_eq!(m.plugin.api_version, workbench_python::HOST_API_VERSION);

    // api_version 不兼容 → 拒绝并给出可读原因（验收 #20）
    let err = workbench_python::parse_manifest(&fixture("bad-version").join("plugin.toml"))
        .err()
        .expect("bad-version 应被拒绝");
    assert!(err.contains("API 版本不兼容"), "原因: {err}");

    // entry_point 拆分
    let (file, func) = workbench_python::split_entry_point("plugin.py:register").unwrap();
    assert_eq!((file.as_str(), func.as_str()), ("plugin.py", "register"));
    assert!(workbench_python::split_entry_point("noseparator").is_err());
}

#[test]
fn 清单_空ID与非法字符被拒绝() {
    let dir = std::env::temp_dir().join("wbpy-manifest-test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("plugin.toml");
    std::fs::write(&path, "[plugin]\nid = \"\"\nname=\"x\"\nversion=\"0\"\napi_version=\"1\"\nentry_point=\"a.py:b\"\n").unwrap();
    let err = workbench_python::parse_manifest(&path).err().unwrap();
    assert!(err.contains("id 不能为空"));
    std::fs::write(&path, "[plugin]\nid = \"有 空 格\"\nname=\"x\"\nversion=\"0\"\napi_version=\"1\"\nentry_point=\"a.py:b\"\n").unwrap();
    let err = workbench_python::parse_manifest(&path).err().unwrap();
    assert!(err.contains("非法字符"));
}

// ============ 发现与加载 ============

fn registry_with(dirs: Vec<PathBuf>) -> wb::Registry {
    let mut reg = wb::Registry::new();
    (workbench_python::plugin_stage(dirs))(&mut reg);
    reg
}

#[test]
fn 加载_好插件注册命令与工具栏() {
    let reg = registry_with(vec![fixtures_dir()]);
    assert!(reg.plugins_loaded.contains(&"test.good".to_string()));
    assert!(reg.command("good.ping").is_some());
    assert!(reg.command("good.long").is_some());
    // 来源标记（验收：插件贡献可追溯）
    assert_eq!(reg.command("good.ping").unwrap().source.as_deref(), Some("test.good"));
    // 工具栏贡献：一个插件组、两个条目
    let group = reg
        .toolbar
        .iter()
        .find(|g| g.id.0 == "good.tools")
        .expect("应创建 good.tools 工具栏组");
    assert_eq!(group.items.len(), 2);
    assert_eq!(group.source.as_deref(), Some("test.good"));
}

#[test]
fn 加载_损坏语法插件被隔离且不影响其他插件() {
    let reg = registry_with(vec![fixtures_dir()]);
    // broken-syntax 被拒绝且给出原因
    let rejected = reg
        .plugins_rejected
        .iter()
        .find(|(id, _)| id == "test.broken")
        .expect("broken-syntax 应被拒绝");
    assert!(rejected.1.contains("失败"), "原因: {}", rejected.1);
    // 同目录的其余插件不受影响
    assert!(reg.plugins_loaded.contains(&"test.good".to_string()));
    assert!(reg.command("good.ping").is_some());
}

#[test]
fn 加载_重复ID后发现的被跳过() {
    // 同一目录出现在两个发现路径 → 第二次因 ID 重复被跳过
    let parent = fixtures_dir();
    let reg = registry_with(vec![parent.clone(), parent]);
    assert_eq!(
        reg.plugins_loaded.iter().filter(|i| *i == "test.good").count(),
        1
    );
    let (_, reason) = reg
        .plugins_rejected
        .iter()
        .find(|(id, _)| id == "test.good")
        .expect("重复的 test.good 应被记录");
    assert!(reason.contains("重复"));
}

// ============ 命令执行（完整运行时） ============

struct NoopWb;
impl wb::Workbench for NoopWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-noop")
    }
    fn init(&self, _reg: &mut wb::Registry) {}
}

fn app_with_plugins() -> AppRuntime {
    WorkbenchAppBuilder::new("test.plugins", "插件测试产品")
        .with_workbench(NoopWb)
        .with_plugin_stage(workbench_python::plugin_stage(vec![fixtures_dir()]))
        .with_layout_path(
            std::env::temp_dir()
                .join("workbench-rs-test")
                .join("layout-py.json"),
        )
        .build()
        .expect("装配失败")
}

fn pump(app: &mut AppRuntime, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        app.frame_tick(0.01);
        if !app.host.tasks.all().any(|t| !t.status.is_terminal()) {
            return true;
        }
        if std::time::Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn 命令_插件同步命令返回备注进历史() {
    let mut a = app_with_plugins();
    a.frame_tick(0.0);
    let inv = a.execute_command_tracked(
        "good.ping",
        &wb::CommandArgs::new().with("name", "tester"),
    );
    assert!(inv.is_ok());
    match &inv.result {
        wb::CommandResult::Done { note } => {
            assert_eq!(note.as_deref(), Some("pong:tester"));
        }
        other => panic!("期望 Done，得到 {other:?}"),
    }
    assert_eq!(inv.record.status, wb::InvocationStatus::Succeeded);
}

#[test]
fn 命令_插件同步命令Python异常转失败() {
    // 插件装载成功，但命令执行时 Python 回调抛异常 → 命令 Failed 且含异常信息
    let root = std::env::temp_dir().join("wbpy-throw");
    let dir = root.join("plugin");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("plugin.toml"),
        "[plugin]\nid = \"test.throw\"\nname=\"T\"\nversion=\"0\"\napi_version=\"1\"\nentry_point=\"main.py:register\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.py"),
        "def register(api):\n    def boom(args):\n        raise ValueError('boom-bang!')\n    api.register_command(id='throw.boom', title='Boom', callback=boom)\n",
    )
    .unwrap();
    let mut a = WorkbenchAppBuilder::new("test.throw", "异常插件产品")
        .with_plugin_stage(workbench_python::plugin_stage(vec![root]))
        .build()
        .unwrap();
    a.frame_tick(0.0);
    assert!(
        a.command_title("throw.boom").is_some(),
        "异常插件应被正常装载（register 本身没出错）"
    );
    let r = a.execute_command("throw.boom", &wb::CommandArgs::default());
    match r {
        wb::CommandResult::Failed { error } => {
            assert!(
                error.contains("boom-bang"),
                "错误应包含 Python 异常信息: {error}"
            );
        }
        other => panic!("期望 Failed，得到 {other:?}"),
    }
}

#[test]
fn 命令_插件后台任务完成与取消() {
    let mut a = app_with_plugins();
    a.frame_tick(0.0);
    // 完成路径
    let inv = a.execute_command_tracked("good.long", &wb::CommandArgs::default());
    let task_id = inv.task_id.expect("后台插件命令应有任务句柄");
    assert!(pump(&mut a, std::time::Duration::from_secs(20)));
    assert_eq!(
        a.host.tasks.get(task_id).unwrap().status,
        wb::TaskStatus::Succeeded
    );
    assert_eq!(a.host.tasks.get(task_id).unwrap().kind, wb::TaskKind::Thread);
    // 取消路径：更长的任务，运行中请求取消
    let root = std::env::temp_dir().join("wbpy-long");
    let dir = root.join("plugin");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("plugin.toml"),
        "[plugin]\nid = \"test.long\"\nname=\"L\"\nversion=\"0\"\napi_version=\"1\"\nentry_point=\"main.py:register\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.py"),
        "def register(api):\n    def slow(task, args):\n        for i in range(600):\n            task.check_cancelled()\n            task.report(i / 600.0, 'slow')\n            import time\n            time.sleep(0.02)\n    api.register_command(id='long.slow', title='Slow', callback=slow, background=True)\n",
    )
    .unwrap();
    let mut b = WorkbenchAppBuilder::new("test.long", "长任务产品")
        .with_plugin_stage(workbench_python::plugin_stage(vec![root]))
        .build()
        .unwrap();
    b.frame_tick(0.0);
    let inv = b.execute_command_tracked("long.slow", &wb::CommandArgs::default());
    let tid = inv
        .task_id
        .unwrap_or_else(|| panic!("后台插件命令应有任务句柄，结果: {:?}", inv.result));
    std::thread::sleep(std::time::Duration::from_millis(300));
    b.host.tasks.request_cancel(tid);
    assert!(pump(&mut b, std::time::Duration::from_secs(20)));
    assert_eq!(
        b.host.tasks.get(tid).unwrap().status,
        wb::TaskStatus::Cancelled
    );
}

#[test]
fn 命令_来源移除_插件卸载清理() {
    let mut reg = registry_with(vec![fixtures_dir()]);
    assert!(reg.command("good.ping").is_some());
    let removed = reg.remove_commands_from_source("test.good");
    assert!(removed.contains(&"good.ping".to_string()));
    assert!(removed.contains(&"good.long".to_string()));
    assert!(reg.command("good.ping").is_none());
    // 仅由该插件贡献的空组被一并清理
    assert!(reg.toolbar.iter().all(|g| g.id.0 != "good.tools"));
}
