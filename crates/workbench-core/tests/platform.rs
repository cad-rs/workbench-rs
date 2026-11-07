//! Integration tests for the workbench-core platform runtime (no window, pure logic paths).

use workbench_api as wb;
use workbench_core::{AppRuntime, WorkbenchAppBuilder};

/// A test Workbench: one command, one panel, one view, one document type.
struct TestWb;

impl wb::Workbench for TestWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-test")
    }
    fn init(&self, reg: &mut wb::Registry) {
        reg.register_command(
            wb::CommandDef::sync("test.cmd", "Test Command", |_ctx, _args| wb::CommandResult::done())
                .hotkey(wb::Hotkey::parse("Ctrl+T").unwrap()),
        );
        reg.register_command(CommandCounter::default_cmd());
        reg.register_view(wb::ViewDef::new("test.view", "Test View", true, || {
            Box::new(TestView)
        }));
        reg.register_panel(wb::PanelDef::new(
            "test.panel",
            "Test Panel",
            wb::DockArea::Left,
            200.0,
            0,
            || Box::new(TestView),
        ));
        reg.register_document_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("test.shapes"),
            title: "Shapes Document".to_string(),
            extensions: vec!["shapes".to_string()],
            create_default: || Box::new(TestDoc),
            serialize: |_| Ok(Vec::new()),
            deserialize: |_| Ok(Box::new(TestDoc)),
            open_view: None,
            undo_hooks: None,
            source: None,
        });
        reg.work_modes = vec!["A".into(), "B".into()];
        reg.default_mode = "A".into();
    }
}

use std::sync::atomic::{AtomicU32, Ordering};
static CMD_RUNS: AtomicU32 = AtomicU32::new(0);

struct CommandCounter;
impl CommandCounter {
    fn default_cmd() -> wb::CommandDef {
        wb::CommandDef::sync("test.counter", "Counter Command", |_ctx, _args| {
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
        "Test View".to_string()
    }
    fn element(
        &mut self,
        _host: &mut dyn wb::AppServices,
    ) -> Option<Box<dyn std::any::Any>> {
        None
    }
}

fn builder() -> WorkbenchAppBuilder {
    WorkbenchAppBuilder::new("test.product", "Test Product")
        .with_workbench(TestWb)
        .with_layout_path(std::env::temp_dir().join("workbench-rs-test").join("layout.json"))
}

/// A workbench contributing a second document type, to exercise the
/// "which one is the default" question.
struct SecondDocWb;

impl wb::Workbench for SecondDocWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-second-doc")
    }
    fn init(&self, reg: &mut wb::Registry) {
        reg.register_document_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("test.second"),
            title: "Second Document".to_string(),
            extensions: vec!["second".to_string()],
            create_default: || Box::new(TestDoc),
            serialize: |_| Ok(Vec::new()),
            deserialize: |_| Ok(Box::new(TestDoc)),
            open_view: None,
            undo_hooks: None,
            source: None,
        });
    }
}

/// Minimal document payload for the second type.
struct TestDoc;
impl wb::DocumentContent for TestDoc {
    fn dup(&self) -> Box<dyn wb::DocumentContent> {
        Box::new(TestDoc)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn app() -> AppRuntime {
    builder().build().expect("assembly failed")
}

#[test]
fn assembly_registers_commands_and_panel_toggles() {
    let a = app();
    assert!(a.command_title("test.cmd").is_some());
    // The platform registers a toggle command per panel
    assert!(a.command_title("app.toggle_panel.test.panel").is_some());
    assert!(a.command_title("app.undo").is_some());
    assert_eq!(a.host.ws.work_mode, "A");
}

fn default_doc_builder(with_second: bool) -> WorkbenchAppBuilder {
    let builder = WorkbenchAppBuilder::new("test.default-doc", "Default Document")
        .with_workbench(TestWb)
        .with_layout_path(std::env::temp_dir().join("workbench-rs-test").join("layout.json"))
        .create_default_document(true);
    if with_second {
        builder.with_workbench(SecondDocWb)
    } else {
        builder
    }
}

#[test]
fn product_names_default_type_among_several_document_types() {
    let active_type = |builder: WorkbenchAppBuilder| {
        builder
            .build()
            .expect("assembly failed")
            .host
            .docs
            .active()
            .map(|d| d.type_id.0.clone())
    };

    // Without a preference the first registered type wins.
    assert_eq!(
        active_type(default_doc_builder(true)),
        Some("test.shapes".to_string())
    );

    // The product can name the default explicitly.
    assert_eq!(
        active_type(default_doc_builder(true).with_default_document_type("test.second")),
        Some("test.second".to_string())
    );

    // An unknown id falls back rather than failing the build.
    assert_eq!(
        active_type(default_doc_builder(false).with_default_document_type("test.missing")),
        Some("test.shapes".to_string())
    );
}

#[test]
fn assembly_rejects_duplicate_command_with_diagnostic() {
    let mut reg = wb::Registry::new();
    reg.register_command(wb::CommandDef::sync("dup.cmd", "A", |_c, _a| wb::CommandResult::done()));
    reg.register_command(wb::CommandDef::sync("dup.cmd", "B", |_c, _a| wb::CommandResult::done()));
    assert_eq!(reg.commands.len(), 1);
    assert!(reg.errors.iter().any(|e| e.contains("dup.cmd")));
}

#[test]
fn command_hotkey_parsing() {
    let hk = wb::Hotkey::parse("Ctrl+Shift+N").unwrap();
    assert!(hk.ctrl && hk.shift && !hk.alt);
    assert_eq!(hk.key, wb::KeyCode::N);
    assert_eq!(hk.to_string(), "Ctrl+Shift+N");
    assert!(wb::Hotkey::parse("Ctrl+NoSuchKey").is_none());
}

#[test]
fn command_runs_through_unified_entry_point() {
    let mut a = app();
    a.frame_tick(0.0);
    let r = a.execute_command("test.counter", &wb::CommandArgs::default());
    assert!(r.is_ok());
    assert_eq!(CMD_RUNS.load(Ordering::SeqCst), 1);
    // Unregistered command
    let r = a.execute_command("no.such.command", &wb::CommandArgs::default());
    assert!(matches!(r, wb::CommandResult::Failed { .. }));
}

#[test]
fn context_panel_toggle_command() {
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
fn context_work_mode_switch() {
    use wb::AppServices as _;
    let mut a = app();
    a.frame_tick(0.0);
    a.set_work_mode("B").unwrap();
    a.frame_tick(0.0);
    assert_eq!(a.snapshot().work_mode, "B");
}

#[test]
fn document_edit_undo_redo_and_revision() {
    let mut a = app();
    a.frame_tick(0.0);
    // Register a document type with content
    a.host
        .docs
        .register_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("test.doc"),
            title: "Test".into(),
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
            undo_hooks: None,
            source: None,
        })
        .unwrap();
    let id = a.host.docs.create(&wb::DocumentTypeId::new("test.doc"), "d").unwrap();
    a.host.docs.edit::<Vec<u32>>(id, "Add one", true, |v| v.push(1)).unwrap();
    a.host.docs.edit::<Vec<u32>>(id, "Add two", true, |v| v.push(2)).unwrap();
    assert_eq!(a.host.docs.get(id).unwrap().revision, 2);
    assert!(a.host.docs.can_undo(id));
    a.host.docs.undo(id).unwrap();
    assert_eq!(a.host.docs.read::<Vec<u32>, usize>(id, |v| v.len()), Some(1));
    a.host.docs.redo(id).unwrap();
    assert_eq!(a.host.docs.read::<Vec<u32>, usize>(id, |v| v.len()), Some(2));
    // A non-undoable operation clears the history
    a.host
        .docs
        .commit(id, "Reset", false, Box::new(Vec::<u32>::new()))
        .unwrap();
    assert!(!a.host.docs.can_undo(id));
}

// ---- Delegated undo (the domain keeps its own history; the platform takes no snapshots) ----

/// A delegated test document: an imperative undo stack.
#[derive(Clone, Default)]
struct DelegatedDoc {
    value: i32,
    undo_stack: Vec<i32>,
    redo_stack: Vec<i32>,
}

impl DelegatedDoc {
    fn apply(&mut self, delta: i32) {
        self.undo_stack.push(self.value);
        self.redo_stack.clear();
        self.value += delta;
    }
}

fn delegated_doc(c: &dyn wb::DocumentContent) -> &DelegatedDoc {
    c.as_any().downcast_ref::<DelegatedDoc>().unwrap()
}

fn delegated_doc_mut(c: &mut dyn wb::DocumentContent) -> &mut DelegatedDoc {
    c.as_any_mut().downcast_mut::<DelegatedDoc>().unwrap()
}

fn delegated_undo(c: &mut dyn wb::DocumentContent) -> Result<String, String> {
    let d = delegated_doc_mut(c);
    let prev = d.undo_stack.pop().ok_or("Nothing to undo")?;
    d.redo_stack.push(d.value);
    d.value = prev;
    Ok("Delegated undo".to_string())
}

fn delegated_redo(c: &mut dyn wb::DocumentContent) -> Result<String, String> {
    let d = delegated_doc_mut(c);
    let next = d.redo_stack.pop().ok_or("Nothing to redo")?;
    d.undo_stack.push(d.value);
    d.value = next;
    Ok("Delegated redo".to_string())
}

#[test]
fn document_delegated_undo() {
    let mut a = app();
    a.frame_tick(0.0);
    a.host
        .docs
        .register_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("test.delegated"),
            title: "Delegated Document".into(),
            extensions: vec!["tdel".into()],
            create_default: || Box::new(DelegatedDoc::default()),
            serialize: |c| {
                serde_json::to_vec(&delegated_doc(c).value).map_err(|e| e.to_string())
            },
            deserialize: |b| {
                let value: i32 = serde_json::from_slice(b).map_err(|e| e.to_string())?;
                Ok(Box::new(DelegatedDoc {
                    value,
                    ..Default::default()
                }))
            },
            open_view: None,
            undo_hooks: Some(wb::UndoHooks {
                undo: delegated_undo,
                redo: delegated_redo,
                can_undo: |c| !delegated_doc(c).undo_stack.is_empty(),
                can_redo: |c| !delegated_doc(c).redo_stack.is_empty(),
            }),
            source: None,
        })
        .unwrap();
    let id = a
        .host
        .docs
        .create(&wb::DocumentTypeId::new("test.delegated"), "d")
        .unwrap();
    a.host
        .docs
        .edit::<DelegatedDoc>(id, "Add value", true, |d| d.apply(5))
        .unwrap();
    a.host
        .docs
        .edit::<DelegatedDoc>(id, "Add value", true, |d| d.apply(7))
        .unwrap();
    assert_eq!(a.host.docs.read::<DelegatedDoc, i32>(id, |d| d.value), Some(12));
    // The platform kept no snapshot history (no visible history label); undo/redo go through
    // the domain hooks
    assert!(a.host.docs.get(id).unwrap().last_action_label().is_none());
    assert!(a.host.docs.can_undo(id));
    a.host.docs.undo(id).unwrap();
    assert_eq!(a.host.docs.read::<DelegatedDoc, i32>(id, |d| d.value), Some(5));
    assert!(a.host.docs.can_redo(id));
    a.host.docs.redo(id).unwrap();
    assert_eq!(a.host.docs.read::<DelegatedDoc, i32>(id, |d| d.value), Some(12));
}

#[test]
fn document_background_commit_revision_check() {
    let mut a = app();
    a.frame_tick(0.0);
    a.host
        .docs
        .register_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("test.doc2"),
            title: "Test".into(),
            extensions: vec!["td2".into()],
            create_default: || Box::new(0u32),
            serialize: |_c| Ok(vec![]),
            deserialize: |_b| Ok(Box::new(0u32)),
            open_view: None,
            undo_hooks: None,
            source: None,
        })
        .unwrap();
    let id = a.host.docs.create(&wb::DocumentTypeId::new("test.doc2"), "d").unwrap();
    let rev = a.host.docs.get(id).unwrap().revision;
    // Correct revision: accepted
    let commit = wb::DocCommit {
        doc_id: id,
        expect_revision: rev,
        label: "Commit".into(),
        undoable: false,
        content: Box::new(7u32),
    };
    assert!(a.host.docs.apply_commit(commit).is_ok());
    // Stale revision: rejected
    let stale = wb::DocCommit {
        doc_id: id,
        expect_revision: rev, // now +1
        label: "Stale commit".into(),
        undoable: false,
        content: Box::new(9u32),
    };
    assert!(a.host.docs.apply_commit(stale).is_err());
    // Closed document: rejected
    let gone = wb::DocCommit {
        doc_id: wb::DocumentId(9999),
        expect_revision: 0,
        label: "Orphan commit".into(),
        undoable: false,
        content: Box::new(1u32),
    };
    assert!(a.host.docs.apply_commit(gone).is_err());
}

#[test]
fn task_progress_cancel_and_final_state() {
    use std::sync::mpsc;
    let (tx2, rx2) = wb::task_channel();
    let mut tasks = wb::TaskManager::new(tx2);

    let spec = wb::TaskSpec {
        title: "Test task".into(),
        cancellable: true,
        input: Box::new(()),
        job: Box::new(move |ctx: &mut wb::TaskCtx| {
            for i in 0..10u32 {
                ctx.check_cancelled()?;
                ctx.report(Some(i as f32 / 10.0), "In progress");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None)
        }),
    };
    let id = tasks.spawn(spec);
    tasks.request_cancel(id);
    // Pump events until the final state
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
        assert!(std::time::Instant::now() < deadline, "the task did not finish in time");
    }
    assert_eq!(tasks.get(id).unwrap().status, wb::TaskStatus::Cancelled);
}

/// With the tick decoupled from rendering, `frame_tick`'s return value is what the frontend
/// schedules frames on.
///
/// The old frontend only looked at `tasks.summary().0 > 0`: the moment a task finished the
/// predicate turned false, the terminal event was never pumped, and the diagnostics panel's
/// "Background tasks running/total" lagged until the next input. This test pins "the tick that
/// takes the terminal event must report changed", and that it returns to false once pumped.
#[test]
fn task_tick_still_reports_redraw_after_task_finishes() {
    let mut a = app();
    assert!(!a.frame_tick(0.0), "an idle tick should not request a redraw");

    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let id = a.host.tasks.spawn(wb::TaskSpec {
        title: "Tick regression task".into(),
        cancellable: false,
        input: Box::new(()),
        job: Box::new(move |_ctx: &mut wb::TaskCtx| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = done_tx.send(());
            Ok(None)
        }),
    });

    assert!(a.frame_tick(0.0), "a tick should request a redraw while a task runs");
    assert_eq!(a.host.tasks.summary().0, 1);
    // Wait until the job itself has finished (UI idle: the terminal event has
    // not been pumped yet).
    //
    // `done_tx` is sent from inside the job closure, which happens-before the
    // TaskManager enqueues the terminal `Finished` event. So `done_rx.recv()`
    // returning does NOT guarantee the event is already in the channel: a
    // single `frame_tick` right after it can legitimately observe an empty
    // channel. That race made this test fail 11 of 15 runs. Pump in a loop
    // until the terminal event is actually taken.
    done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the task should finish");
    assert_eq!(
        a.host.tasks.summary().0, 1,
        "until the event is pumped the task still shows as running (the lag's precondition)"
    );
    // While the running count is non-zero `frame_tick` always returns true, so
    // the loop can only observe `summary == 0` on a tick that took the event
    // (`pump_task_events` returned true).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let changed = a.frame_tick(0.0);
        if a.host.tasks.summary().0 == 0 {
            assert!(changed, "the tick that takes the terminal event must report a redraw");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the terminal event was not pumped in time"
        );
    }
    assert_eq!(a.host.tasks.summary().0, 0, "terminal event pumped, running count should be 0");
    assert_eq!(
        a.host.tasks.get(id).unwrap().status,
        wb::TaskStatus::Succeeded
    );
    assert!(!a.frame_tick(0.0), "an idle tick after pumping should not request a redraw");
}

#[test]
fn workspace_singleton_view_and_layout_restore() {
    let mut a = app();
    // Singleton view: opening it again reuses the same instance
    let first = a.host.ws.open_view(&wb::ViewTypeId::new("test.view"), None, None).unwrap();
    let second = a.host.ws.open_view(&wb::ViewTypeId::new("test.view"), None, None).unwrap();
    assert_eq!(first, second);
    // Layout: an injected unknown panel → skipped
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
fn layout_core_save_load_round_trip() {
    let dir = std::env::temp_dir().join("workbench-rs-test");
    let _ = std::fs::create_dir_all(&dir);
    let mut a = builder()
        .with_layout_path(dir.join("roundtrip.json"))
        .build()
        .unwrap();
    let ui = serde_json::json!({ "adapter": "test" });
    let dock = serde_json::json!({ "center": { "panel_name": "StackPanel" } });
    assert_eq!(a.save_layout(ui.clone(), Some(dock.clone())), Ok(true));
    assert_eq!(a.save_layout(ui, Some(dock)), Ok(false), "an unchanged layout isn't rewritten");
    let mut b = builder()
        .with_layout_path(dir.join("roundtrip.json"))
        .build()
        .unwrap();
    let restored = b.load_layout().expect("layout restored");
    assert!(restored.ui.is_some());
    assert_eq!(restored.ui.unwrap()["adapter"], "test");
    // The dock blob rides along untouched: the core never parses it.
    let dock = restored.dock.expect("dock layout restored");
    assert_eq!(dock["center"]["panel_name"], "StackPanel");
}

// ===================== Phase 2: async commands and unified invocation results ==========

struct AsyncTestWb;

impl wb::Workbench for AsyncTestWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-async-test")
    }
    fn init(&self, reg: &mut wb::Registry) {
        reg.register_command(wb::CommandDef::sync("t.ping", "Sync", |_c, _a| {
            wb::CommandResult::done()
        }));
        reg.register_command(wb::CommandDef::async_command("t.async", "Test Async", |_ctx| {
            Ok(wb::AsyncSpec {
                title: "Test async".into(),
                cancellable: true,
                input: Box::new(()),
                job: Box::new(|task: wb::TaskCtx| {
                    Box::pin(async move {
                        for i in 0..5u32 {
                            task.check_cancelled()?;
                            task.report(Some(i as f32 / 5.0), "Step");
                            async_io::Timer::after(std::time::Duration::from_millis(20)).await;
                        }
                        Ok(None)
                    })
                }),
            })
        }));
        reg.register_command(wb::CommandDef::async_command(
            "t.async_long",
            "Cancellable Async",
            |_ctx| {
                Ok(wb::AsyncSpec {
                    title: "Cancellable async".into(),
                    cancellable: true,
                    input: Box::new(()),
                    job: Box::new(|task: wb::TaskCtx| {
                        Box::pin(async move {
                            for i in 0..100u32 {
                                task.check_cancelled()?;
                                task.report(Some(i as f32 / 100.0), "Long task");
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
    WorkbenchAppBuilder::new("test.async", "Async Test Product")
        .with_workbench(AsyncTestWb)
        .with_layout_path(std::env::temp_dir().join("workbench-rs-test").join("layout-async.json"))
        .build()
        .expect("assembly failed")
}

/// Pump frame_tick until no task is running.
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
fn phase2_async_command_completes_and_records_final_state() {
    let mut a = async_app();
    a.frame_tick(0.0);
    let inv = a.execute_command_tracked("t.async", &wb::CommandArgs::default());
    assert!(inv.is_ok());
    let task_id = inv.task_id.expect("an async invocation should return a task handle");
    assert!(inv.record.status == wb::InvocationStatus::Running);
    assert!(pump(&mut a, std::time::Duration::from_secs(10)));
    assert_eq!(a.host.tasks.get(task_id).unwrap().status, wb::TaskStatus::Succeeded);
    assert_eq!(a.host.tasks.get(task_id).unwrap().kind, wb::TaskKind::Async);
    // The task's terminal event advanced the invocation record
    let rec = a
        .recent_invocations()
        .find(|r| r.task_id == Some(task_id))
        .expect("there should be an invocation record");
    assert_eq!(rec.status, wb::InvocationStatus::Succeeded);
    assert_eq!(rec.execution, Some(wb::CommandExecution::Async));
}

#[test]
fn phase2_async_command_cooperative_cancel() {
    let mut a = async_app();
    a.frame_tick(0.0);
    let inv = a.execute_command_tracked("t.async_long", &wb::CommandArgs::default());
    let task_id = inv.task_id.expect("task handle");
    std::thread::sleep(std::time::Duration::from_millis(120));
    a.host.tasks.request_cancel(task_id);
    assert!(pump(&mut a, std::time::Duration::from_secs(10)));
    assert_eq!(a.host.tasks.get(task_id).unwrap().status, wb::TaskStatus::Cancelled);
    let rec = a
        .recent_invocations()
        .find(|r| r.task_id == Some(task_id))
        .expect("there should be an invocation record");
    assert_eq!(rec.status, wb::InvocationStatus::Cancelled);
}

#[test]
fn phase2_executor_runs_several_async_commands_concurrently() {
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
    assert!(done, "all concurrent async tasks should succeed");
}

#[test]
fn phase2_invocation_history_ring_capacity_and_unregistered_record() {
    let mut a = async_app();
    a.frame_tick(0.0);
    for _ in 0..105 {
        let _ = a.execute_command("t.ping", &wb::CommandArgs::default());
    }
    assert_eq!(a.recent_invocations().count(), 100, "the history should be a ring buffer");
    let inv = a.execute_command_tracked("no.such", &wb::CommandArgs::default());
    assert!(!inv.is_ok());
    assert_eq!(inv.record.status, wb::InvocationStatus::Failed);
    assert_eq!(inv.record.execution, None);
}

#[test]
fn phase2_document_revision_query() {
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
            undo_hooks: None,
            source: None,
        })
        .unwrap();
    let id = a.host.docs.create(&wb::DocumentTypeId::new("t.doc"), "d").unwrap();
    assert_eq!(a.host.docs.revision(id), Some(0));
    a.host.docs.edit::<u32>(id, "Change", true, |v| *v += 1).unwrap();
    assert_eq!(a.host.docs.revision(id), Some(1));
    assert_eq!(a.host.docs.revision(wb::DocumentId(4242)), None, "no such document");
}

// ===================== RibbonBar（design.md §7） =====================

struct RibbonWb;

impl wb::Workbench for RibbonWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-ribbon")
    }
    fn init(&self, reg: &mut wb::Registry) {
        // Explicit tabs: Home (merged with the platform default) + Tools
        reg.add_ribbon_tab(
            wb::RibbonTab::new("app.home", "Home")
                .from_source("wb-ribbon")
                .groups(vec![
                    wb::ToolbarGroup::new("r.file", "File").items(vec![
                        wb::ToolbarItem::command("app.new_document"),
                    ]),
                    wb::ToolbarGroup::new("r.edit", "Edit").items(vec![
                        wb::ToolbarItem::command("app.undo"),
                    ]),
                ]),
        );
        reg.add_ribbon_tab(
            wb::RibbonTab::new("wb.tools", "Tools")
                .groups(vec![
                    wb::ToolbarGroup::new("r.demo", "Demo").items(vec![
                        wb::ToolbarItem::command("test.cmd"),
                    ]),
                ]),
        );
        // Legacy group → default tab
        reg.add_toolbar_group(
            wb::ToolbarGroup::new("r.legacy", "Legacy")
                .from_source("wb-ribbon")
                .items(vec![wb::ToolbarItem::command("test.cmd")]),
        );
    }
}

fn ribbon_app() -> AppRuntime {
    WorkbenchAppBuilder::new("test.ribbon", "Ribbon Test Product")
        .with_workbench(RibbonWb)
        .with_workbench(TestWb)
        .with_layout_path(std::env::temp_dir().join("workbench-rs-test").join("layout-rb.json"))
        .build()
        .expect("assembly failed")
}

#[test]
fn ribbon_tab_merging_and_legacy_compatibility() {
    let a = ribbon_app();
    let ws = &a.host.ws;
    assert!(ws.ribbon.iter().any(|t| t.id.0 == "app.home"));
    assert!(ws.ribbon.iter().any(|t| t.id.0 == "wb.tools"));
    let home = ws.ribbon.iter().find(|t| t.id.0 == "app.home").unwrap();
    let ids: Vec<&str> = home.groups.iter().map(|g| g.id.0.as_str()).collect();
    // Legacy groups merge into the default tab; the platform's "View" group takes the legacy
    // path too
    assert!(ids.contains(&"r.legacy"), "groups: {ids:?}");
    assert!(ids.contains(&"app.window"), "groups: {ids:?}");
    // The Home tab is active by default
    assert_eq!(ws.active_ribbon_tab, "app.home");
}

#[test]
fn ribbon_active_tab_switch_and_layout_persistence() {
    let mut a = ribbon_app();
    {
        let ws = &mut a.host.ws;
        ws.set_active_ribbon_tab("wb.tools");
        assert_eq!(ws.active_ribbon_tab, "wb.tools");
        let saved = ws.save_layout();
        assert_eq!(saved.ribbon_active_tab.as_deref(), Some("wb.tools"));
        // An injected unregistered tab → falls back to the first tab with a diagnostic (§6.5)
        let mut saved2 = ws.save_layout();
        saved2.ribbon_active_tab = Some("ghost.tab".to_string());
        let skipped = ws.apply_layout(&saved2);
        assert!(skipped.iter().any(|m| m.contains("ghost.tab")));
        assert_eq!(ws.active_ribbon_tab, "app.home", "falls back to the first tab");
        // Normal restore
        let skipped = ws.apply_layout(&saved);
        assert!(skipped.is_empty());
        assert_eq!(ws.active_ribbon_tab, "wb.tools");
    }
}

/// A storage provider in memory, standing in for OPFS / a cloud store.
#[derive(Default)]
struct MemoryStorage {
    files: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
    dirs: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl MemoryStorage {
    fn with_files(files: &[&str]) -> Self {
        let storage = Self::default();
        for key in files {
            storage.files.lock().unwrap().insert(key.to_string(), Vec::new());
        }
        storage
    }
}

impl wb::StorageProvider for MemoryStorage {
    fn label(&self) -> &str {
        "Memory"
    }
    fn read(&self, key: &str) -> wb::StorageFuture<Vec<u8>> {
        let file = self.files.lock().unwrap().get(key).cloned();
        Box::pin(std::future::ready(file.ok_or_else(|| "No such file".to_string())))
    }
    fn write(&self, key: &str, bytes: Vec<u8>) -> wb::StorageFuture<()> {
        self.files.lock().unwrap().insert(key.to_string(), bytes);
        Box::pin(std::future::ready(Ok(())))
    }
    fn list(&self, dir: &str) -> wb::StorageFuture<Vec<wb::Entry>> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let dirs = self.dirs.lock().unwrap();
        let mut listing = std::collections::BTreeMap::new();
        for key in self.files.lock().unwrap().keys().chain(dirs.iter()) {
            let Some(rest) = key.strip_prefix(&prefix).filter(|rest| !rest.is_empty()) else {
                continue;
            };
            let (name, is_dir) = match rest.split_once('/') {
                Some((first, _)) => (first, true),
                None => (rest, dirs.contains(key)),
            };
            let key = format!("{prefix}{name}");
            listing.insert(name.to_string(), wb::Entry { key, name: name.to_string(), is_dir });
        }
        Box::pin(std::future::ready(Ok(listing.into_values().collect())))
    }
    fn create_dir(&self, key: &str) -> wb::StorageFuture<()> {
        self.dirs.lock().unwrap().insert(key.to_string());
        Box::pin(std::future::ready(Ok(())))
    }
}

/// Pump frame_tick until the results of in-flight file reads/writes are all applied.
fn pump_files(app: &mut AppRuntime) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.pending_io() > 0 {
        assert!(std::time::Instant::now() < deadline, "file I/O timed out");
        app.frame_tick(0.01);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn storage_save_through_provider_and_reopen() {
    let memory = std::sync::Arc::new(MemoryStorage::default());
    let mut app = builder().with_storage("mem", memory.clone()).build().expect("assembly failed");
    let doc = app.host.docs.create(&wb::DocumentTypeId::new("test.shapes"), "Untitled").unwrap();
    app.host.docs.commit(doc, "edit", true, Box::new(TestDoc)).unwrap();

    let location = wb::Location::new("mem", "projects/part.shapes");
    let started = workbench_core::platform_cmds::save_document(
        &mut wb::CommandCtx::new(&mut app),
        doc,
        Some(location.clone()),
    );
    assert!(!matches!(started, wb::CommandResult::Failed { .. }));
    assert!(app.host.docs.get(doc).unwrap().dirty, "still unsaved until the write completes");
    pump_files(&mut app);
    assert!(memory.files.lock().unwrap().contains_key("projects/part.shapes"));
    let saved = app.host.docs.get(doc).unwrap();
    assert_eq!(saved.title, "part");
    assert_eq!(saved.location.as_ref(), Some(&location));
    assert!(!saved.dirty);

    let mut ctx = wb::CommandCtx::new(&mut app);
    workbench_core::platform_cmds::open_document(&mut ctx, location.clone());
    pump_files(&mut app);
    let opened = app.host.docs.active().expect("the opened document becomes active");
    assert_ne!(opened.id, doc);
    assert_eq!(opened.location.as_ref(), Some(&location));
}

#[test]
fn storage_unregistered_provider_logs_error() {
    let mut app = app();
    workbench_core::platform_cmds::open_document(
        &mut wb::CommandCtx::new(&mut app),
        wb::Location::new("nowhere", "a.shapes"),
    );
    pump_files(&mut app);
    assert!(!app.host.docs.docs().any(|d| d.location.is_some()));
    assert!(app
        .host
        .log
        .entries()
        .any(|e| e.message.contains("no `nowhere` storage")));
}

#[test]
fn autosave_recovers_after_crash_and_clears_after_clean_exit() {
    let dir = std::env::temp_dir().join(format!("workbench-rs-autosave-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let session = || builder().with_layout_path(dir.join("layout.json")).build().unwrap();

    // Session 1 autosaves a dirty document, then crashes: no clean-exit marker.
    let mut a = session();
    let doc = a.host.docs.create(&wb::DocumentTypeId::new("test.shapes"), "Draft").unwrap();
    a.host.docs.commit(doc, "edit", true, Box::new(TestDoc)).unwrap();
    assert_eq!(a.autosave_now(), Ok(1));
    let state = a.state().clone();
    drop(a);
    state.remove("autosave/session.clean");

    // Session 2 recovers it (dirty, no file), autosaves again, exits cleanly.
    let mut b = session();
    let recovered: Vec<_> = b.host.docs.docs().filter(|d| d.title == "[Recovered] Draft").collect();
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].dirty && recovered[0].location.is_none());
    assert_eq!(b.autosave_now(), Ok(1));
    drop(b);

    // Session 3 follows a clean exit: nothing is recovered, and session 2's
    // autosave is cleared rather than kept for some later crash.
    let c = session();
    assert!(!c.host.docs.docs().any(|d| d.title.starts_with("[Recovered]")));
    assert!(c.state().keys("autosave/").is_empty());
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The names in the open picker's listing (folders end in `/`).
fn listing(app: &AppRuntime) -> Vec<String> {
    let picker = app.host.ws.file_picker.as_ref().expect("picker open");
    let entries = picker.listing.as_ref().expect("listed").as_ref().expect("listing ok");
    entries
        .iter()
        .map(|e| if e.is_dir { format!("{}/", e.name) } else { e.name.clone() })
        .collect()
}

#[test]
fn storage_in_app_picker_open_and_save_as() {
    use workbench_core::file_picker;

    let memory = std::sync::Arc::new(MemoryStorage::with_files(&[
        "projects/part.shapes",
        "projects/notes.txt",
        "top.shapes",
    ]));
    // A provider without a native dialog: open/save use the in-app picker.
    let mut app = builder().with_storage("mem", memory.clone()).build().expect("assembly failed");

    // Open: the picker lists the root, folders first, files filtered by type.
    app.execute_command("app.open_document", &wb::CommandArgs::default());
    pump_files(&mut app);
    assert_eq!(listing(&app), ["projects/", "top.shapes"]);
    let picker = app.host.ws.file_picker.as_ref().unwrap();
    assert_eq!(picker.sources, [("mem".to_string(), "Memory".to_string())]);
    let folder = picker.listing.as_ref().unwrap().as_ref().unwrap()[0].clone();
    file_picker::navigate(&mut wb::CommandCtx::new(&mut app), |p| p.enter(&folder));
    pump_files(&mut app);
    assert_eq!(listing(&app), ["part.shapes"], "notes.txt is filtered out");
    let part = wb::Location::new("mem", "projects/part.shapes");
    app.finish_pick(Some(part.clone()));
    pump_files(&mut app);
    assert!(app.host.ws.file_picker.is_none());
    let opened = app.host.docs.active().expect("opened");
    assert_eq!((opened.title.as_str(), opened.location.as_ref()), ("part", Some(&part)));

    // Save as: a new folder through the picker's prompt command, then a
    // typed name, completed with the type's extension.
    app.execute_command("app.save_document_as", &wb::CommandArgs::default());
    pump_files(&mut app);
    let picker = app.host.ws.file_picker.as_ref().expect("save picker");
    assert_eq!(picker.save.as_deref(), Some("part.shapes"));
    let created = app.execute_command(
        file_picker::NEW_FOLDER,
        &wb::CommandArgs::new().with("value", "copies"),
    );
    assert!(!matches!(created, wb::CommandResult::Failed { .. }), "{created:?}");
    pump_files(&mut app);
    assert_eq!(listing(&app), ["copies/", "projects/", "top.shapes"]);
    let target = file_picker::target(&mut wb::CommandCtx::new(&mut app), "copy").unwrap();
    assert_eq!(target, wb::Location::new("mem", "copy.shapes"));
    app.finish_pick(Some(target.clone()));
    pump_files(&mut app);
    assert!(memory.files.lock().unwrap().contains_key("copy.shapes"));
    assert_eq!(app.host.docs.active().unwrap().location.as_ref(), Some(&target));

    // Cancel closes the picker and runs nothing.
    app.execute_command("app.open_document", &wb::CommandArgs::default());
    pump_files(&mut app);
    let docs = app.host.docs.docs().count();
    app.finish_pick(None);
    pump_files(&mut app);
    assert!(app.host.ws.file_picker.is_none());
    assert_eq!(app.host.docs.docs().count(), docs);
}
