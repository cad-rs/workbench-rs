//! Performance regression tests (phase 5, §18: more thorough automated and performance tests).
//! The thresholds are loose (covering debug-build jitter): the aim is to catch
//! order-of-magnitude regressions, not precise benchmarks.

use std::time::Instant;

use workbench_api as wb;
use workbench_core::{AppRuntime, WorkbenchAppBuilder};

struct PerfWb;

impl wb::Workbench for PerfWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-perf")
    }
    fn init(&self, reg: &mut wb::Registry) {
        reg.register_command(wb::CommandDef::sync("p.noop", "No-op Command", |_c, _a| {
            wb::CommandResult::done()
        }));
        reg.register_command(
            wb::CommandDef::sync("p.add", "Add One", |ctx: &mut wb::CommandCtx, _a| {
                let Some(doc) = ctx.active_document() else {
                    return wb::CommandResult::failed("No document");
                };
                ctx.host()
                    .docs
                    .edit::<Vec<u32>>(doc, "Add one", true, |v| v.push(v.len() as u32))
                    .map(|_| wb::CommandResult::done())
                    .unwrap_or_else(wb::CommandResult::failed)
            })
            .enabled_when(|snap| snap.active_document.is_some()),
        );
        reg.register_document_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("p.doc"),
            title: "Perf".into(),
            extensions: vec!["pdoc".into()],
            create_default: || Box::new(Vec::<u32>::new()),
            serialize: |c| {
                serde_json::to_vec(c.as_any().downcast_ref::<Vec<u32>>().unwrap())
                    .map_err(|e| e.to_string())
            },
            deserialize: |b| {
                serde_json::from_slice::<Vec<u32>>(b)
                    .map(|v: Vec<u32>| Box::new(v) as Box<dyn wb::DocumentContent>)
                    .map_err(|e| e.to_string())
            },
            open_view: None,
            undo_hooks: None,
            source: None,
        });
    }
}

fn perf_app() -> AppRuntime {
    WorkbenchAppBuilder::new("test.perf", "Perf Test Product")
        .with_workbench(PerfWb)
        .with_layout_path(
            std::env::temp_dir()
                .join("workbench-rs-perf")
                .join("layout.json"),
        )
        .build()
        .expect("assembly failed")
}

#[test]
fn perf_sync_command_dispatch_throughput() {
    let mut a = perf_app();
    a.frame_tick(0.0);
    const N: u32 = 10_000;
    let start = Instant::now();
    for _ in 0..N {
        let r = a.execute_command("p.noop", &wb::CommandArgs::default());
        debug_assert!(r.is_ok());
    }
    let elapsed = start.elapsed();
    println!(
        "perf: {N} sync command dispatches took {:?} ({:.2} µs each)",
        elapsed,
        elapsed.as_micros() as f64 / N as f64
    );
    // With snapshot construction + snapshot cache hits, one dispatch should not exceed 2ms
    // (debug build)
    assert!(
        elapsed.as_secs_f64() < N as f64 * 0.002,
        "sync command dispatch regressed: {elapsed:?}"
    );
}

#[test]
fn perf_large_document_edit_and_undo() {
    let mut a = perf_app();
    a.frame_tick(0.0);
    // One commit builds a large document of 20k elements
    let doc = a
        .host
        .docs
        .create(&wb::DocumentTypeId::new("p.doc"), "Large document")
        .unwrap();
    a.host.docs.set_active(Some(doc));
    a.host
        .docs
        .edit::<Vec<u32>>(doc, "Generate data", true, |v| {
            v.extend(0..20_000u32);
        })
        .unwrap();

    // 200 undoable edits (two snapshot dups each)
    let start = Instant::now();
    for _ in 0..200 {
        a.host
            .docs
            .edit::<Vec<u32>>(doc, "Edit", true, |v| v.push(1))
            .unwrap();
    }
    let edits = start.elapsed();

    // Undo + redo (history capacity is 100; each runs the full capacity)
    let start = Instant::now();
    let mut undos = 0;
    while a.host.docs.can_undo(doc) {
        a.host.docs.undo(doc).unwrap();
        undos += 1;
    }
    let mut redos = 0;
    while a.host.docs.can_redo(doc) {
        a.host.docs.redo(doc).unwrap();
        redos += 1;
    }
    let undo_redo = start.elapsed();

    println!(
        "perf: 200 edits (20k document) {edits:?}, {undos} undos + {redos} redos {undo_redo:?}"
    );
    assert!(edits.as_secs() < 30, "edit performance regressed: {edits:?}");
    assert!(undo_redo.as_secs() < 30, "undo/redo performance regressed: {undo_redo:?}");
}

#[test]
fn perf_background_task_pump_throughput() {
    let mut a = perf_app();
    a.frame_tick(0.0);
    // 100 lightweight background tasks
    for i in 0..100u32 {
        a.host.tasks.spawn(wb::TaskSpec {
            title: format!("Task {i}"),
            cancellable: false,
            input: Box::new(()),
            job: Box::new(|_ctx: &mut wb::TaskCtx| {
                std::thread::sleep(std::time::Duration::from_millis(1));
                Ok(None)
            }),
        });
    }
    let start = Instant::now();
    let deadline = start + std::time::Duration::from_secs(60);
    loop {
        a.frame_tick(0.005);
        if !a.host.tasks.all().any(|t| !t.status.is_terminal()) {
            break;
        }
        assert!(Instant::now() < deadline, "the task pump timed out");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let elapsed = start.elapsed();
    println!("perf: all 100 background tasks finished in {elapsed:?}");
    assert!(elapsed.as_secs() < 30, "task pump performance regressed: {elapsed:?}");
}
