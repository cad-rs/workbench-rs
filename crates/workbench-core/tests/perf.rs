//! 性能回归测试（阶段 5，§18：更完善的自动化和性能测试）。
//! 断言阈值宽松（覆盖 debug 构建的抖动），目的是捕捉量级退化而非精确基准。

use std::time::Instant;

use workbench_api as wb;
use workbench_core::{AppRuntime, WorkbenchAppBuilder};

struct PerfWb;

impl wb::Workbench for PerfWb {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-perf")
    }
    fn init(&self, reg: &mut wb::Registry) {
        reg.register_command(wb::CommandDef::sync("p.noop", "空命令", |_c, _a| {
            wb::CommandResult::done()
        }));
        reg.register_command(
            wb::CommandDef::sync("p.add", "加一", |ctx: &mut wb::CommandCtx, _a| {
                let Some(doc) = ctx.active_document() else {
                    return wb::CommandResult::failed("无文档");
                };
                ctx.host()
                    .docs
                    .edit::<Vec<u32>>(doc, "加一", true, |v| v.push(v.len() as u32))
                    .map(|_| wb::CommandResult::done())
                    .unwrap_or_else(wb::CommandResult::failed)
            })
            .enabled_when(|snap| snap.active_document.is_some()),
        );
        reg.register_document_type(wb::DocumentTypeDef {
            id: wb::DocumentTypeId::new("p.doc"),
            title: "性能".into(),
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
        });
    }
}

fn perf_app() -> AppRuntime {
    WorkbenchAppBuilder::new("test.perf", "性能测试产品")
        .with_workbench(PerfWb)
        .with_layout_path(
            std::env::temp_dir()
                .join("workbench-rs-perf")
                .join("layout.json"),
        )
        .build()
        .expect("装配失败")
}

#[test]
fn perf_同步命令分发吞吐() {
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
        "perf: {N} 次同步命令分发耗时 {:?}（{:.2} µs/次）",
        elapsed,
        elapsed.as_micros() as f64 / N as f64
    );
    // 快照构造 + 快照缓存命中下，单次分发不应超过 2ms（debug 构建）
    assert!(
        elapsed.as_secs_f64() < N as f64 * 0.002,
        "同步命令分发退化: {elapsed:?}"
    );
}

#[test]
fn perf_大文档编辑与撤销() {
    let mut a = perf_app();
    a.frame_tick(0.0);
    // 一次提交造出 20k 元素的大文档
    let doc = a
        .host
        .docs
        .create(&wb::DocumentTypeId::new("p.doc"), "大文档")
        .unwrap();
    a.host.docs.set_active(Some(doc));
    a.host
        .docs
        .edit::<Vec<u32>>(doc, "造数据", true, |v| {
            v.extend(0..20_000u32);
        })
        .unwrap();

    // 200 次可撤销编辑（每次两份快照 dup）
    let start = Instant::now();
    for _ in 0..200 {
        a.host
            .docs
            .edit::<Vec<u32>>(doc, "编辑", true, |v| v.push(1))
            .unwrap();
    }
    let edits = start.elapsed();

    // 撤销 + 重做（历史容量上限 100，各按容量走满）
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
        "perf: 200 次编辑（20k 文档）{edits:?}，{undos} 撤销+{redos} 重做 {undo_redo:?}"
    );
    assert!(edits.as_secs() < 30, "编辑性能退化: {edits:?}");
    assert!(undo_redo.as_secs() < 30, "撤销/重做性能退化: {undo_redo:?}");
}

#[test]
fn perf_后台任务泵吞吐() {
    let mut a = perf_app();
    a.frame_tick(0.0);
    // 100 个轻量后台任务
    for i in 0..100u32 {
        a.host.tasks.spawn(wb::TaskSpec {
            title: format!("任务{i}"),
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
        assert!(Instant::now() < deadline, "任务泵超时");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let elapsed = start.elapsed();
    println!("perf: 100 个后台任务全部完成耗时 {elapsed:?}");
    assert!(elapsed.as_secs() < 30, "任务泵性能退化: {elapsed:?}");
}
