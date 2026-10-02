//! workbench-core 平台运行时集成测试（无窗口，纯逻辑路径）。

use workbench_api as wb;
use workbench_core::{AppRuntime, WorkbenchAppBuilder};

/// 测试用 Workbench：一个命令、一个面板、一个视图、一个文档类型。
struct TestWb;

impl wb::Workbench for TestWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-test")
    }
    fn init(&self, reg: &mut wb::Registry) {
        reg.register_command(
            wb::CommandDef::sync("test.cmd", "测试命令", |_ctx, _args| wb::CommandResult::done())
                .hotkey(wb::Hotkey::parse("Ctrl+T").unwrap()),
        );
        reg.register_command(CommandCounter::default_cmd());
        reg.register_view(wb::ViewDef::new("test.view", "测试视图", true, || {
            Box::new(TestView)
        }));
        reg.register_panel(wb::PanelDef::new(
            "test.panel",
            "测试面板",
            wb::DockArea::Left,
            200.0,
            0,
            || Box::new(TestView),
        ));
        reg.work_modes = vec!["A".into(), "B".into()];
        reg.default_mode = "A".into();
    }
}

use std::sync::atomic::{AtomicU32, Ordering};
static CMD_RUNS: AtomicU32 = AtomicU32::new(0);

struct CommandCounter;
impl CommandCounter {
    fn default_cmd() -> wb::CommandDef {
        wb::CommandDef::sync("test.counter", "计数命令", |_ctx, _args| {
            CMD_RUNS.fetch_add(1, Ordering::SeqCst);
            wb::CommandResult::done()
        })
    }
}

struct TestView;

impl wb::ViewInstance for TestView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("test.view")
    }
    fn title(&self) -> String {
        "测试视图".to_string()
    }
    fn paint(&mut self, p: &mut dyn wb::PaintBackend, rect: wb::Rect, _ctx: &mut wb::ViewCtx) {
        p.fill_rect(rect, wb::Color::GRAY, 0.0);
    }
}

fn builder() -> WorkbenchAppBuilder {
    WorkbenchAppBuilder::new("test.product", "测试产品")
        .with_workbench(TestWb)
        .with_layout_path(std::env::temp_dir().join("workbench-rs-test").join("layout.json"))
}

fn app() -> AppRuntime {
    builder().build().expect("装配失败")
}

#[test]
fn 装配_注册与面板命令() {
    let a = app();
    assert!(a.command_title("test.cmd").is_some());
    // 平台为每个面板注册了 toggle 命令
    assert!(a.command_title("app.toggle_panel.test.panel").is_some());
    assert!(a.command_title("app.undo").is_some());
    assert_eq!(a.host.ws.work_mode, "A");
}

#[test]
fn 装配_重复命令被拒绝并记录诊断() {
    let mut reg = wb::Registry::new();
    reg.register_command(wb::CommandDef::sync("dup.cmd", "A", |_c, _a| wb::CommandResult::done()));
    reg.register_command(wb::CommandDef::sync("dup.cmd", "B", |_c, _a| wb::CommandResult::done()));
    assert_eq!(reg.commands.len(), 1);
    assert!(reg.errors.iter().any(|e| e.contains("dup.cmd")));
}

#[test]
fn 命令_快捷键解析() {
    let hk = wb::Hotkey::parse("Ctrl+Shift+N").unwrap();
    assert!(hk.ctrl && hk.shift && !hk.alt);
    assert_eq!(hk.key, wb::KeyCode::N);
    assert_eq!(hk.to_string(), "Ctrl+Shift+N");
    assert!(wb::Hotkey::parse("Ctrl+不存在的键").is_none());
}

#[test]
fn 命令_统一入口执行() {
    let mut a = app();
    a.frame_tick(0.0);
    let r = a.execute_command("test.counter", &wb::CommandArgs::default());
    assert!(r.is_ok());
    assert_eq!(CMD_RUNS.load(Ordering::SeqCst), 1);
    // 未注册命令
    let r = a.execute_command("no.such.command", &wb::CommandArgs::default());
    assert!(matches!(r, wb::CommandResult::Failed { .. }));
}

#[test]
fn 上下文_面板显隐命令() {
    let mut a = app();
    a.frame_tick(0.0);
    let vis = |a: &AppRuntime| {
        a.host.ws
            .panels
            .iter()
            .find(|p| p.id.0 == "test.panel")
            .map(|p| p.visible)
            .unwrap_or(false)
    };
    let visible_before = vis(&a);
    let r = a.execute_command("app.toggle_panel.test.panel", &wb::CommandArgs::default());
    assert!(r.is_ok());
    let visible_after = vis(&a);
    assert_ne!(visible_before, visible_after);
}

#[test]
fn 上下文_工作模式切换() {
    use wb::AppServices as _;
    let mut a = app();
    a.frame_tick(0.0);
    a.set_work_mode("B").unwrap();
    a.frame_tick(0.0);
    assert_eq!(a.snapshot().work_mode, "B");
}

#[test]
fn 文档_编辑撤销重做与修订号() {
    let mut a = app();
    a.frame_tick(0.0);
    // 注册一个带内容的文档类型
    a.host
        .docs
        .register_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("test.doc"),
            title: "测试".into(),
            extensions: vec!["tdoc".into()],
            create_default: || Box::new(Vec::<u32>::new()),
            serialize: |c| {
                serde_json::to_vec(c.as_any().downcast_ref::<Vec<u32>>().unwrap())
                    .map_err(|e| e.to_string())
            },
            deserialize: |b| {
                serde_json::from_slice::<Vec<u32>>(b)
                    .map(Box::new)
                    .map(|v: Box<Vec<u32>>| v as Box<dyn wb::DocumentContent>)
                    .map_err(|e| e.to_string())
            },
            open_view: None,
        })
        .unwrap();
    let id = a.host.docs.create(&wb::DocumentTypeId::new("test.doc"), "d").unwrap();
    a.host.docs.edit::<Vec<u32>>(id, "加一", true, |v| v.push(1)).unwrap();
    a.host.docs.edit::<Vec<u32>>(id, "加二", true, |v| v.push(2)).unwrap();
    assert_eq!(a.host.docs.get(id).unwrap().revision, 2);
    assert!(a.host.docs.can_undo(id));
    a.host.docs.undo(id).unwrap();
    assert_eq!(a.host.docs.read::<Vec<u32>, usize>(id, |v| v.len()), Some(1));
    a.host.docs.redo(id).unwrap();
    assert_eq!(a.host.docs.read::<Vec<u32>, usize>(id, |v| v.len()), Some(2));
    // 不可撤销操作清空历史
    a.host
        .docs
        .commit(id, "重置", false, Box::new(Vec::<u32>::new()))
        .unwrap();
    assert!(!a.host.docs.can_undo(id));
}

#[test]
fn 文档_后台提交版本校验() {
    let mut a = app();
    a.frame_tick(0.0);
    a.host
        .docs
        .register_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("test.doc2"),
            title: "测试".into(),
            extensions: vec!["td2".into()],
            create_default: || Box::new(0u32),
            serialize: |_c| Ok(vec![]),
            deserialize: |_b| Ok(Box::new(0u32)),
            open_view: None,
        })
        .unwrap();
    let id = a.host.docs.create(&wb::DocumentTypeId::new("test.doc2"), "d").unwrap();
    let rev = a.host.docs.get(id).unwrap().revision;
    // 正确版本：接受
    let commit = wb::DocCommit {
        doc_id: id,
        expect_revision: rev,
        label: "提交".into(),
        undoable: false,
        content: Box::new(7u32),
    };
    assert!(a.host.docs.apply_commit(commit).is_ok());
    // 过期版本：拒绝
    let stale = wb::DocCommit {
        doc_id: id,
        expect_revision: rev, // 当前已 +1
        label: "过期提交".into(),
        undoable: false,
        content: Box::new(9u32),
    };
    assert!(a.host.docs.apply_commit(stale).is_err());
    // 已关闭文档：拒绝
    let gone = wb::DocCommit {
        doc_id: wb::DocumentId(9999),
        expect_revision: 0,
        label: "孤儿提交".into(),
        undoable: false,
        content: Box::new(1u32),
    };
    assert!(a.host.docs.apply_commit(gone).is_err());
}

#[test]
fn 任务_进度取消与终态() {
    use std::sync::mpsc;
    let (tx2, rx2) = wb::task_channel();
    let mut tasks = wb::TaskManager::new(tx2);

    let spec = wb::TaskSpec {
        title: "测试任务".into(),
        cancellable: true,
        input: Box::new(()),
        job: Box::new(move |ctx: &mut wb::TaskCtx| {
            for i in 0..10u32 {
                ctx.check_cancelled()?;
                ctx.report(Some(i as f32 / 10.0), "进行中");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None)
        }),
    };
    let id = tasks.spawn(spec);
    tasks.request_cancel(id);
    // 泵事件直至终态
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(info) = tasks.get(id) {
            if info.status.is_terminal() {
                break;
            }
        }
        match rx2.lock().unwrap().recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(ev) => {
                tasks.apply_event(&ev);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        assert!(std::time::Instant::now() < deadline, "任务未在时限内结束");
    }
    assert_eq!(tasks.get(id).unwrap().status, wb::TaskStatus::Cancelled);
}

#[test]
fn 工作区_单例视图与布局恢复() {
    let mut a = app();
    // 单例视图：重复打开复用同一实例
    let first = a.host.ws.open_view(&wb::ViewTypeId::new("test.view"), None, None).unwrap();
    let second = a.host.ws.open_view(&wb::ViewTypeId::new("test.view"), None, None).unwrap();
    assert_eq!(first, second);
    // 布局：注入未知面板 → 跳过
    let mut layout = a.host.ws.save_layout();
    layout.panels.push(wb::SavedPanel {
        id: "ghost".into(),
        area: wb::DockArea::Right,
        order: 9,
        size: 100.0,
        visible: true,
    });
    let skipped = a.host.ws.apply_layout(&layout);
    assert!(skipped.iter().any(|m| m.contains("ghost")));
    assert!(!a.host.ws.tabs.is_empty());
}

#[test]
fn 布局_核心保存加载往返() {
    let dir = std::env::temp_dir().join("workbench-rs-test");
    let _ = std::fs::create_dir_all(&dir);
    let a = builder()
        .with_layout_path(dir.join("roundtrip.json"))
        .build()
        .unwrap();
    a.save_layout(serde_json::json!({ "adapter": "test" })).unwrap();
    let mut b = builder()
        .with_layout_path(dir.join("roundtrip.json"))
        .build()
        .unwrap();
    let ui = b.load_layout();
    assert!(ui.is_some());
    assert_eq!(ui.unwrap()["adapter"], "test");
}

// ===================== 阶段 2：异步命令与统一调用结果 =====================

struct AsyncTestWb;

impl wb::Workbench for AsyncTestWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-async-test")
    }
    fn init(&self, reg: &mut wb::Registry) {
        reg.register_command(wb::CommandDef::sync("t.ping", "同步", |_c, _a| {
            wb::CommandResult::done()
        }));
        reg.register_command(wb::CommandDef::async_command("t.async", "测试异步", |_ctx| {
            Ok(wb::AsyncSpec {
                title: "测试异步".into(),
                cancellable: true,
                input: Box::new(()),
                job: Box::new(|task: wb::TaskCtx| {
                    Box::pin(async move {
                        for i in 0..5u32 {
                            task.check_cancelled()?;
                            task.report(Some(i as f32 / 5.0), "步进");
                            async_io::Timer::after(std::time::Duration::from_millis(20)).await;
                        }
                        Ok(None)
                    })
                }),
            })
        }));
        reg.register_command(wb::CommandDef::async_command(
            "t.async_long",
            "可取消异步",
            |_ctx| {
                Ok(wb::AsyncSpec {
                    title: "可取消异步".into(),
                    cancellable: true,
                    input: Box::new(()),
                    job: Box::new(|task: wb::TaskCtx| {
                        Box::pin(async move {
                            for i in 0..100u32 {
                                task.check_cancelled()?;
                                task.report(Some(i as f32 / 100.0), "长任务");
                                async_io::Timer::after(std::time::Duration::from_millis(50)).await;
                            }
                            Ok(None)
                        })
                    }),
                })
            },
        ));
    }
}

fn async_app() -> AppRuntime {
    WorkbenchAppBuilder::new("test.async", "异步测试产品")
        .with_workbench(AsyncTestWb)
        .with_layout_path(std::env::temp_dir().join("workbench-rs-test").join("layout-async.json"))
        .build()
        .expect("装配失败")
}

/// 泵 frame_tick 直到没有运行中的任务。
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
fn 阶段2_异步命令执行完成且记录终态() {
    let mut a = async_app();
    a.frame_tick(0.0);
    let inv = a.execute_command_tracked("t.async", &wb::CommandArgs::default());
    assert!(inv.is_ok());
    let task_id = inv.task_id.expect("异步调用应返回任务句柄");
    assert!(inv.record.status == wb::InvocationStatus::Running);
    assert!(pump(&mut a, std::time::Duration::from_secs(10)));
    assert_eq!(a.host.tasks.get(task_id).unwrap().status, wb::TaskStatus::Succeeded);
    assert_eq!(a.host.tasks.get(task_id).unwrap().kind, wb::TaskKind::Async);
    // 调用记录被任务终态事件推进
    let rec = a
        .recent_invocations()
        .find(|r| r.task_id == Some(task_id))
        .expect("应有调用记录");
    assert_eq!(rec.status, wb::InvocationStatus::Succeeded);
    assert_eq!(rec.execution, Some(wb::CommandExecution::Async));
}

#[test]
fn 阶段2_异步命令协作取消() {
    let mut a = async_app();
    a.frame_tick(0.0);
    let inv = a.execute_command_tracked("t.async_long", &wb::CommandArgs::default());
    let task_id = inv.task_id.expect("任务句柄");
    std::thread::sleep(std::time::Duration::from_millis(120));
    a.host.tasks.request_cancel(task_id);
    assert!(pump(&mut a, std::time::Duration::from_secs(10)));
    assert_eq!(a.host.tasks.get(task_id).unwrap().status, wb::TaskStatus::Cancelled);
    let rec = a
        .recent_invocations()
        .find(|r| r.task_id == Some(task_id))
        .expect("应有调用记录");
    assert_eq!(rec.status, wb::InvocationStatus::Cancelled);
}

#[test]
fn 阶段2_执行器并发处理多个异步命令() {
    let mut a = async_app();
    a.frame_tick(0.0);
    for _ in 0..4 {
        let r = a.execute_command("t.async", &wb::CommandArgs::default());
        assert!(r.is_ok());
    }
    assert!(pump(&mut a, std::time::Duration::from_secs(20)));
    let done = a
        .host
        .tasks
        .all()
        .filter(|t| t.kind == wb::TaskKind::Async)
        .all(|t| t.status == wb::TaskStatus::Succeeded);
    assert!(done, "并发异步任务应全部成功");
}

#[test]
fn 阶段2_调用历史环形容量与未注册命令记录() {
    let mut a = async_app();
    a.frame_tick(0.0);
    for _ in 0..105 {
        let _ = a.execute_command("t.ping", &wb::CommandArgs::default());
    }
    assert_eq!(a.recent_invocations().count(), 100, "历史应为环形缓冲");
    let inv = a.execute_command_tracked("no.such", &wb::CommandArgs::default());
    assert!(!inv.is_ok());
    assert_eq!(inv.record.status, wb::InvocationStatus::Failed);
    assert_eq!(inv.record.execution, None);
}

#[test]
fn 阶段2_文档修订号查询() {
    let mut a = async_app();
    a.frame_tick(0.0);
    a.host
        .docs
        .register_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("t.doc"),
            title: "T".into(),
            extensions: vec!["tx".into()],
            create_default: || Box::new(0u32),
            serialize: |_| Ok(vec![]),
            deserialize: |_| Ok(Box::new(0u32)),
            open_view: None,
        })
        .unwrap();
    let id = a.host.docs.create(&wb::DocumentTypeId::new("t.doc"), "d").unwrap();
    assert_eq!(a.host.docs.revision(id), Some(0));
    a.host.docs.edit::<u32>(id, "改", true, |v| *v += 1).unwrap();
    assert_eq!(a.host.docs.revision(id), Some(1));
    assert_eq!(a.host.docs.revision(wb::DocumentId(4242)), None, "不存在的文档");
}

// ===================== RibbonBar（design.md §7） =====================

struct RibbonWb;

impl wb::Workbench for RibbonWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-ribbon")
    }
    fn init(&self, reg: &mut wb::Registry) {
        // 显式 Tab：主页（合并平台默认）+ 工具
        reg.add_ribbon_tab(
            wb::RibbonTab::new("app.home", "主页")
                .from_source("wb-ribbon")
                .groups(vec![
                    wb::ToolbarGroup::new("r.file", "文件").items(vec![
                        wb::ToolbarItem::command("app.new_document"),
                    ]),
                    wb::ToolbarGroup::new("r.edit", "编辑").items(vec![
                        wb::ToolbarItem::command("app.undo"),
                    ]),
                ]),
        );
        reg.add_ribbon_tab(
            wb::RibbonTab::new("wb.tools", "工具")
                .groups(vec![
                    wb::ToolbarGroup::new("r.demo", "演示").items(vec![
                        wb::ToolbarItem::command("test.cmd"),
                    ]),
                ]),
        );
        // legacy 组 → 默认 Tab
        reg.add_toolbar_group(
            wb::ToolbarGroup::new("r.legacy", "遗留")
                .from_source("wb-ribbon")
                .items(vec![wb::ToolbarItem::command("test.cmd")]),
        );
    }
}

fn ribbon_app() -> AppRuntime {
    WorkbenchAppBuilder::new("test.ribbon", "Ribbon 测试产品")
        .with_workbench(RibbonWb)
        .with_workbench(TestWb)
        .with_layout_path(std::env::temp_dir().join("workbench-rs-test").join("layout-rb.json"))
        .build()
        .expect("装配失败")
}

#[test]
fn ribbon_tab合并与legacy兼容() {
    let a = ribbon_app();
    let ws = &a.host.ws;
    assert!(ws.ribbon.iter().any(|t| t.id.0 == "app.home"));
    assert!(ws.ribbon.iter().any(|t| t.id.0 == "wb.tools"));
    let home = ws.ribbon.iter().find(|t| t.id.0 == "app.home").unwrap();
    let ids: Vec<&str> = home.groups.iter().map(|g| g.id.0.as_str()).collect();
    // legacy 组并入默认 Tab；平台「窗口」组也走 legacy 路径
    assert!(ids.contains(&"r.legacy"), "groups: {ids:?}");
    assert!(ids.contains(&"app.window"), "groups: {ids:?}");
    // 主页 Tab 活动为默认
    assert_eq!(ws.active_ribbon_tab, "app.home");
}

#[test]
fn ribbon_活动tab切换与布局持久化() {
    let mut a = ribbon_app();
    {
        let ws = &mut a.host.ws;
        ws.set_active_ribbon_tab("wb.tools");
        assert_eq!(ws.active_ribbon_tab, "wb.tools");
        let saved = ws.save_layout();
        assert_eq!(saved.ribbon_active_tab.as_deref(), Some("wb.tools"));
        // 注入未注册 Tab → 回落首个 Tab 并给出诊断（§6.5）
        let mut saved2 = ws.save_layout();
        saved2.ribbon_active_tab = Some("ghost.tab".to_string());
        let skipped = ws.apply_layout(&saved2);
        assert!(skipped.iter().any(|m| m.contains("ghost.tab")));
        assert_eq!(ws.active_ribbon_tab, "app.home", "回落首个 Tab");
        // 正常恢复
        let skipped = ws.apply_layout(&saved);
        assert!(skipped.is_empty());
        assert_eq!(ws.active_ribbon_tab, "wb.tools");
    }
}
