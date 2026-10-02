//! wb-example：验证 workbench-rs 平台的示例 Workbench 模块。
//!
//! 只依赖 workbench-api，不依赖任何 GUI 框架（design.md 验收 #21）。

mod shapes;
mod views;

use workbench_api as wb;
use wb::{
    CommandArgs, CommandCtx, CommandDef, CommandResult, DocumentTypeDef, DockArea, PanelDef,
    Registry, RibbonTab, ToolbarGroup, ToolbarItem, ViewDef,
};

use shapes::ShapeDoc;
use views::ExampleState;

pub struct ExampleWorkbench;

impl wb::Workbench for ExampleWorkbench {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-example")
    }

    fn init(&self, reg: &mut Registry) {
        // ---- 文档类型 ----
        reg.register_document_type(DocumentTypeDef {
            id: wb::DocumentTypeId::new("example.shapes"),
            title: "图形文档".to_string(),
            extensions: vec!["shapes".to_string()],
            create_default: || Box::new(ShapeDoc::default()),
            serialize: shapes::serialize,
            deserialize: shapes::deserialize,
            open_view: Some(wb::ViewTypeId::new("example.canvas")),
        });

        // ---- 中央视图 ----
        reg.register_view(ViewDef::new("example.welcome", "欢迎", true, || {
            Box::new(views::WelcomeView)
        }));
        reg.register_view(ViewDef::new("example.canvas", "图形画布", true, || {
            Box::new(views::CanvasView)
        }));

        // ---- Dock 面板 ----
        reg.register_panel(PanelDef::new(
            "example.panel.explorer",
            "资源管理器",
            DockArea::Left,
            240.0,
            0,
            || Box::new(views::ExplorerPanel),
        ));
        reg.register_panel(PanelDef::new(
            "example.panel.tasks",
            "任务",
            DockArea::Right,
            300.0,
            0,
            || Box::new(views::TasksPanel),
        ));
        reg.register_panel(PanelDef::new(
            "example.panel.properties",
            "属性",
            DockArea::Right,
            300.0,
            1,
            || Box::new(views::PropertiesPanel),
        ));
        reg.register_panel(PanelDef::new(
            "example.panel.output",
            "输出",
            DockArea::Bottom,
            170.0,
            0,
            || Box::new(views::OutputPanel),
        ));

        // ---- 扩展服务 ----
        reg.add_service(ExampleState::default());

        // ---- 工作模式 ----
        reg.work_modes = vec!["建模".to_string(), "草图".to_string()];
        reg.default_mode = "建模".to_string();

        register_commands(reg);
        register_toolbar(reg);
    }
}

const CANVAS: &str = "example.canvas";

fn register_commands(reg: &mut Registry) {
    // 打开画布（绑定活动文档）
    reg.register_command(CommandDef::sync(
        "example.open_canvas",
        "打开画布",
        |ctx: &mut CommandCtx, _a: &CommandArgs| match ctx.open_view(CANVAS, true) {
            Ok(_) => CommandResult::done(),
            Err(e) => CommandResult::failed(e),
        },
    ));

    // 打开欢迎页
    reg.register_command(CommandDef::sync(
        "example.open_welcome",
        "欢迎页",
        |ctx: &mut CommandCtx, _a: &CommandArgs| match ctx.open_view("example.welcome", false) {
            Ok(_) => CommandResult::done(),
            Err(e) => CommandResult::failed(e),
        },
    ));

    // 添加图形（同步、可撤销；上下文：画布视图 + 活动文档）
    reg.register_command(
        CommandDef::sync(
            "example.add_shape",
            "添加图形",
            |ctx: &mut CommandCtx, _a: &CommandArgs| {
                let Some(doc) = ctx.active_document() else {
                    return CommandResult::failed("没有活动文档");
                };
                match ctx.host().docs.edit::<ShapeDoc>(doc, "添加图形", true, |s| {
                    s.add_shape();
                }) {
                    Ok(_) => {
                        let last = ctx
                            .host()
                            .docs
                            .read::<ShapeDoc, usize>(doc, |s| s.shapes.len().saturating_sub(1));
                        if let Some(i) = last {
                            if let Some(state) = ctx.host().service_mut::<ExampleState>() {
                                state.selected = Some((doc, i));
                            }
                        }
                        CommandResult::done()
                    }
                    Err(e) => CommandResult::failed(e),
                }
            },
        )
        .enabled_when(|snap| snap.active_document.is_some() && snap.active_view_is(CANVAS)),
    );

    // 删除选中图形
    reg.register_command(
        CommandDef::sync(
            "example.remove_selected",
            "删除选中图形",
            |ctx: &mut CommandCtx, _a: &CommandArgs| {
                let Some(doc) = ctx.active_document() else {
                    return CommandResult::failed("没有活动文档");
                };
                let sel = ctx
                    .host()
                    .service::<ExampleState>()
                    .and_then(|s| s.selected)
                    .filter(|(d, _)| *d == doc)
                    .map(|(_, i)| i);
                let Some(index) = sel else {
                    return CommandResult::failed("没有选中的图形");
                };
                let ok = ctx.host().docs.edit::<ShapeDoc>(doc, "删除图形", true, |s| {
                    if index < s.shapes.len() {
                        s.shapes.remove(index);
                    }
                });
                match ok {
                    Ok(_) => {
                        if let Some(state) = ctx.host().service_mut::<ExampleState>() {
                            state.selected = None;
                        }
                        CommandResult::done()
                    }
                    Err(e) => CommandResult::failed(e),
                }
            },
        )
        .enabled_when(|snap| {
            snap.active_document.is_some()
                && snap.active_view_is(CANVAS)
                && snap.active_document.as_ref().map(|d| d.revision > 0).unwrap_or(false)
        }),
    );

    // 画布点击选中（内部命令：由视图触发，演示统一命令入口）
    reg.register_command(
        CommandDef::sync(
            "example.select_shape",
            "选择图形",
            |ctx: &mut CommandCtx, args: &CommandArgs| {
                let Some(doc) = ctx.active_document() else {
                    return CommandResult::failed("没有活动文档");
                };
                let index: Option<usize> = args.parse("index");
                if let Some(state) = ctx.host().service_mut::<ExampleState>() {
                    state.selected = index.map(|i| (doc, i));
                }
                CommandResult::done()
            },
        )
        .enabled_when(|snap| snap.active_document.is_some() && snap.active_view_is(CANVAS)),
    );

    // 批量生成（后台任务：进度 + 协作式取消 + 版本校验提交）
    reg.register_command(
        CommandDef::background(
            "example.generate_shapes",
            "批量生成图形",
            |ctx: &mut CommandCtx| {
                let Some(doc) = ctx.active_document() else {
                    return Err("没有活动文档".to_string());
                };
                let revision = ctx
                    .host()
                    .docs
                    .get(doc)
                    .map(|d| d.revision)
                    .ok_or("文档不存在")?;
                let base = ctx
                    .host()
                    .docs
                    .read::<ShapeDoc, ShapeDoc>(doc, |s| s.clone())
                    .ok_or("文档内容不是 ShapeDoc")?;
                Ok(wb::TaskSpec {
                    title: "批量生成图形".to_string(),
                    cancellable: true,
                    input: Box::new(GenerateInput {
                        doc_id: doc,
                        revision,
                        base,
                    }),
                    job: Box::new(|task: &mut wb::TaskCtx| {
                        let Some(input) = task.input::<GenerateInput>() else {
                            return Err(wb::TaskFailure::Error("缺少任务输入".to_string()));
                        };
                        let input = input.clone();
                        let steps = 8u32;
                        let mut doc = input.base;
                        for step in 0..steps {
                            task.check_cancelled()?;
                            task.report(
                                Some(step as f32 / steps as f32),
                                &format!("生成批次 {}/{}", step + 1, steps),
                            );
                            doc.append(25);
                            std::thread::sleep(std::time::Duration::from_millis(120));
                        }
                        Ok(Some(wb::DocCommit {
                            doc_id: input.doc_id,
                            expect_revision: input.revision,
                            label: "批量生成图形".to_string(),
                            undoable: true,
                            content: Box::new(doc),
                        }))
                    }),
                })
            },
        )
        .enabled_when(|snap| snap.active_document.is_some()),
    );

    // 异步扫描（阶段 2：共享执行器上的轻量异步任务，不独占线程）
    reg.register_command(CommandDef::async_command(
        "example.async_scan",
        "异步扫描（执行器）",
        |_ctx: &mut CommandCtx| {
            Ok(wb::AsyncSpec {
                title: "异步扫描".to_string(),
                cancellable: true,
                input: Box::new(()),
                job: Box::new(|task: wb::TaskCtx| {
                    Box::pin(async move {
                        let steps = 12u32;
                        for step in 0..steps {
                            task.check_cancelled()?;
                            task.report(
                                Some(step as f32 / steps as f32),
                                &format!("扫描片段 {}/{}", step + 1, steps),
                            );
                            async_io::Timer::after(std::time::Duration::from_millis(100)).await;
                        }
                        task.log("异步扫描完成（共享执行器线程池）");
                        Ok(None)
                    })
                }),
            })
        },
    ));

    // 慢任务（后台、可取消、无文档提交）
    reg.register_command(CommandDef::background(
        "example.slow_job",
        "慢任务（演示进度与取消）",
        |_ctx: &mut CommandCtx| {
            Ok(wb::TaskSpec {
                title: "慢任务".to_string(),
                cancellable: true,
                input: Box::new(()),
                job: Box::new(|task: &mut wb::TaskCtx| {
                    let steps = 25u32;
                    for step in 0..steps {
                        task.check_cancelled()?;
                        task.report(
                            Some(step as f32 / steps as f32),
                            &format!("步骤 {}/{}", step + 1, steps),
                        );
                        if step == 10 {
                            task.log("已完成一半，试试取消按钮");
                        }
                        std::thread::sleep(std::time::Duration::from_millis(120));
                    }
                    Ok(None)
                }),
            })
        },
    ));

    // 失败演示
    reg.register_command(CommandDef::sync(
        "example.failing_cmd",
        "失败演示",
        |_ctx: &mut CommandCtx, _a: &CommandArgs| {
            CommandResult::failed("演示：命令执行失败（见输出面板与状态栏）")
        },
    ));

    // 切换工作模式
    reg.register_command(CommandDef::sync(
        "example.toggle_mode",
        "切换工作模式",
        |ctx: &mut CommandCtx, _a: &CommandArgs| {
            let next = if ctx.snapshot().work_mode == "建模" {
                "草图"
            } else {
                "建模"
            };
            match ctx.set_work_mode(next) {
                Ok(_) => CommandResult::done(),
                Err(e) => CommandResult::failed(e),
            }
        },
    ));
}

fn register_toolbar(reg: &mut Registry) {
    // RibbonBar（design.md §7）：主页 Tab 与平台默认 Tab 合并，工具 Tab 收纳视图与演示
    reg.add_ribbon_tab(
        RibbonTab::new("app.home", "主页")
            .from_source("wb-example")
            .groups(vec![
                ToolbarGroup::new("example.file", "文件")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::command("app.new_document"),
                        ToolbarItem::command("app.open_document"),
                        ToolbarItem::command("app.save_document"),
                    ]),
                ToolbarGroup::new("example.edit", "编辑")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::command("app.undo"),
                        ToolbarItem::command("app.redo"),
                    ]),
                ToolbarGroup::new("example.shapes", "图形")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::command("example.add_shape"),
                        ToolbarItem::command("example.remove_selected"),
                        ToolbarItem::labeled("example.generate_shapes", "批量生成(后台)"),
                    ]),
            ]),
    );
    reg.add_ribbon_tab(
        RibbonTab::new("wb.example.tools", "工具")
            .from_source("wb-example")
            .groups(vec![
                ToolbarGroup::new("example.view", "视图")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::labeled("example.open_canvas", "画布"),
                        ToolbarItem::labeled("example.open_welcome", "欢迎页"),
                        ToolbarItem::labeled("example.toggle_mode", "切换模式"),
                    ]),
                ToolbarGroup::new("example.demo", "演示")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::labeled("example.async_scan", "异步扫描(执行器)"),
                        ToolbarItem::labeled("example.slow_job", "慢任务(可取消)"),
                        ToolbarItem::labeled("example.failing_cmd", "失败演示"),
                    ]),
            ]),
    );
}

/// 后台任务输入（UI 线程收集，任务线程消费）。
#[derive(Clone)]
struct GenerateInput {
    doc_id: wb::DocumentId,
    revision: u64,
    base: ShapeDoc,
}
