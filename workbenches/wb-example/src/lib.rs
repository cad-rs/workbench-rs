//! wb-example: the example Workbench module that exercises the workbench-rs platform.
//!
//! Depends on workbench-api, plus gpui for the content of its views.

mod shapes;
mod views;

use workbench_api as wb;
use wb::{
    ComboState, CommandArgs, CommandCtx, CommandDef, CommandResult, DockArea, PanelDef, Registry,
    RibbonTab, ToolbarGroup, ToolbarItem, ViewDef,
};

use shapes::ShapeDoc;
use views::ExampleState;

pub struct ExampleWorkbench;

impl wb::Workbench for ExampleWorkbench {
    fn id(&self) -> wb::WorkbenchId {
        wb::WorkbenchId::new("wb-example")
    }

    fn init(&self, reg: &mut Registry) {
        // ---- Document types ----
        reg.register_document_type(shapes::document_type());

        // ---- Central views ----
        reg.register_view(
            ViewDef::new("example.welcome", "Welcome", true, || {
                Box::new(views::WelcomeView)
            })
            .icon(icons::WELCOME),
        );
        reg.register_view(
            ViewDef::new("example.canvas", "Shape Canvas", true, || {
                Box::new(views::CanvasView)
            })
            .icon(icons::CANVAS),
        );

        // ---- Dock panels ----
        reg.register_panel(
            PanelDef::new(
                "example.panel.explorer",
                "Explorer",
                DockArea::Left,
                240.0,
                0,
                || Box::new(views::ExplorerPanel),
            )
            .icon(icons::EXPLORER),
        );
        reg.register_panel(
            PanelDef::new(
                "example.panel.tasks",
                "Tasks",
                DockArea::Right,
                300.0,
                0,
                || Box::new(views::TasksPanel),
            )
            .icon(icons::TASKS),
        );
        reg.register_panel(
            PanelDef::new(
                "example.panel.properties",
                "Properties",
                DockArea::Right,
                300.0,
                1,
                || Box::new(views::PropertiesPanel),
            )
            .icon(icons::PROPERTIES),
        );
        reg.register_panel(
            PanelDef::new(
                "example.panel.output",
                "Output",
                DockArea::Bottom,
                170.0,
                0,
                || Box::new(views::OutputPanel),
            )
            .icon(icons::OUTPUT),
        );

        // ---- Extension services ----
        reg.add_service(ExampleState::default());

        // ---- Work modes ----
        reg.work_modes = vec!["Modeling".to_string(), "Sketch".to_string()];
        reg.default_mode = "Modeling".to_string();

        register_commands(reg);
        register_toolbar(reg);

        // ---- zh-CN translations (English UI strings are the keys) ----
        reg.register_locales(
            "zh-CN",
            [
                // Document type.
                ("Shape Document", "图形文档"),
                // Views and panels.
                ("Welcome", "欢迎"),
                ("Shape Canvas", "图形画布"),
                ("Explorer", "资源管理器"),
                ("Tasks", "任务"),
                ("Properties", "属性"),
                ("Output", "输出"),
                // Work modes.
                ("Modeling", "建模"),
                ("Sketch", "草图"),
                // Commands.
                ("Open Canvas", "打开画布"),
                ("Welcome Page", "欢迎页"),
                ("Add Shape", "添加图形"),
                ("Delete Selected Shape", "删除选中图形"),
                ("Select Shape", "选择图形"),
                ("Batch Generate Shapes", "批量生成图形"),
                ("Async Scan (Executor)", "异步扫描（执行器）"),
                ("Slow Task (Progress and Cancel Demo)", "慢任务（演示进度与取消）"),
                ("Failure Demo", "失败演示"),
                ("Toggle Work Mode", "切换工作模式"),
                ("Lineweight", "线宽"),
                // Ribbon tabs and groups.
                ("Home", "主页"),
                ("File", "文件"),
                // Large file buttons: short labels fit the 48px ribbon column.
                ("New", "新建"),
                ("Edit", "编辑"),
                ("Shapes", "图形"),
                ("Attributes", "特性"),
                ("Tools", "工具"),
                ("Views", "视图"),
                ("Demo", "演示"),
                // Ribbon item labels.
                ("Batch Generate (Background)", "批量生成(后台)"),
                ("Sketch Mode", "草图模式"),
                ("Canvas", "画布"),
                ("View", "窗口"),
                ("Async Scan", "异步扫描"),
                ("Slow Task (Cancellable)", "慢任务(可取消)"),
            ],
        );
    }
}

const CANVAS: &str = "example.canvas";

// ---- Icon assets ----

/// Icon paths shared by the platform, views and panels (a static embedded table under
/// `assets/icons/`).
pub mod icons {
    pub const ADD: &str = "icons/add.svg";
    pub const DELETE: &str = "icons/delete.svg";
    pub const GENERATE: &str = "icons/generate.svg";
    pub const CANVAS: &str = "icons/canvas.svg";
    pub const WELCOME: &str = "icons/welcome.svg";
    pub const MODE: &str = "icons/mode.svg";
    pub const SCAN: &str = "icons/scan.svg";
    pub const CLOCK: &str = "icons/clock.svg";
    pub const WARNING: &str = "icons/warning.svg";
    pub const EXPLORER: &str = "icons/explorer.svg";
    pub const TASKS: &str = "icons/tasks.svg";
    pub const PROPERTIES: &str = "icons/properties.svg";
    pub const OUTPUT: &str = "icons/output.svg";
}

/// The example asset source (embedded with `include_bytes!`). Products mount it at assembly
/// time: `builder.with_assets(wb_example::example_assets())`.
pub fn example_assets() -> impl wb::AssetSource {
    static ASSETS: wb::StaticAssets = wb::StaticAssets::new(&[
        (icons::ADD, include_bytes!("../assets/icons/add.svg")),
        (icons::DELETE, include_bytes!("../assets/icons/delete.svg")),
        (icons::GENERATE, include_bytes!("../assets/icons/generate.svg")),
        (icons::CANVAS, include_bytes!("../assets/icons/canvas.svg")),
        (icons::WELCOME, include_bytes!("../assets/icons/welcome.svg")),
        (icons::MODE, include_bytes!("../assets/icons/mode.svg")),
        (icons::SCAN, include_bytes!("../assets/icons/scan.svg")),
        (icons::CLOCK, include_bytes!("../assets/icons/clock.svg")),
        (icons::WARNING, include_bytes!("../assets/icons/warning.svg")),
        (icons::EXPLORER, include_bytes!("../assets/icons/explorer.svg")),
        (icons::TASKS, include_bytes!("../assets/icons/tasks.svg")),
        (icons::PROPERTIES, include_bytes!("../assets/icons/properties.svg")),
        (icons::OUTPUT, include_bytes!("../assets/icons/output.svg")),
    ]);
    ASSETS
}

fn register_commands(reg: &mut Registry) {
    // Open the canvas (bound to the active document)
    reg.register_command(
        CommandDef::sync(
            "example.open_canvas",
            "Open Canvas",
            |ctx: &mut CommandCtx, _a: &CommandArgs| match ctx.open_view(CANVAS, true) {
                Ok(_) => CommandResult::done(),
                Err(e) => CommandResult::failed(e),
            },
        )
        .icon(icons::CANVAS),
    );

    // Open the welcome page
    reg.register_command(
        CommandDef::sync(
            "example.open_welcome",
            "Welcome Page",
            |ctx: &mut CommandCtx, _a: &CommandArgs| match ctx.open_view("example.welcome", false) {
                Ok(_) => CommandResult::done(),
                Err(e) => CommandResult::failed(e),
            },
        )
        .icon(icons::WELCOME),
    );

    // Add a shape (sync, undoable; context: canvas view + active document)
    reg.register_command(
        CommandDef::sync(
            "example.add_shape",
            "Add Shape",
            |ctx: &mut CommandCtx, _a: &CommandArgs| {
                let Some(doc) = ctx.active_document() else {
                    return CommandResult::failed("No active document");
                };
                match ctx.host().docs.edit::<ShapeDoc>(doc, "Add Shape", true, |s| {
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
        .icon(icons::ADD)
        .enabled_when(|snap| snap.active_document.is_some() && snap.active_view_is(CANVAS)),
    );

    // Delete the selected shape
    reg.register_command(
        CommandDef::sync(
            "example.remove_selected",
            "Delete Selected Shape",
            |ctx: &mut CommandCtx, _a: &CommandArgs| {
                let Some(doc) = ctx.active_document() else {
                    return CommandResult::failed("No active document");
                };
                let sel = ctx
                    .host()
                    .service::<ExampleState>()
                    .and_then(|s| s.selected)
                    .filter(|(d, _)| *d == doc)
                    .map(|(_, i)| i);
                let Some(index) = sel else {
                    return CommandResult::failed("No shape selected");
                };
                let ok = ctx.host().docs.edit::<ShapeDoc>(doc, "Delete Shape", true, |s| {
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
        .icon(icons::DELETE)
        .enabled_when(|snap| {
            snap.active_document.is_some()
                && snap.active_view_is(CANVAS)
                && snap.active_document.as_ref().map(|d| d.revision > 0).unwrap_or(false)
        }),
    );

    // Click-to-select on the canvas (internal command: fired by the view, demonstrating the
    // single command entry point)
    reg.register_command(
        CommandDef::sync(
            "example.select_shape",
            "Select Shape",
            |ctx: &mut CommandCtx, args: &CommandArgs| {
                let Some(doc) = ctx.active_document() else {
                    return CommandResult::failed("No active document");
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

    // Batch generate (background task: progress + cooperative cancellation + revision-checked
    // commit)
    reg.register_command(
        CommandDef::background(
            "example.generate_shapes",
            "Batch Generate Shapes",
            |ctx: &mut CommandCtx| {
                let Some(doc) = ctx.active_document() else {
                    return Err("No active document".to_string());
                };
                let revision = ctx
                    .host()
                    .docs
                    .get(doc)
                    .map(|d| d.revision)
                    .ok_or("Document does not exist")?;
                let base = ctx
                    .host()
                    .docs
                    .read::<ShapeDoc, ShapeDoc>(doc, |s| s.clone())
                    .ok_or("Document content is not a ShapeDoc")?;
                Ok(wb::TaskSpec {
                    title: "Batch Generate Shapes".to_string(),
                    cancellable: true,
                    input: Box::new(GenerateInput {
                        doc_id: doc,
                        revision,
                        base,
                    }),
                    job: Box::new(|task: &mut wb::TaskCtx| {
                        let Some(input) = task.input::<GenerateInput>() else {
                            return Err(wb::TaskFailure::Error("Missing task input".to_string()));
                        };
                        let input = input.clone();
                        let steps = 8u32;
                        let mut doc = input.base;
                        for step in 0..steps {
                            task.check_cancelled()?;
                            task.report(
                                Some(step as f32 / steps as f32),
                                &format!("Generating batch {}/{}", step + 1, steps),
                            );
                            doc.append(25);
                            std::thread::sleep(std::time::Duration::from_millis(120));
                        }
                        Ok(Some(wb::DocCommit {
                            doc_id: input.doc_id,
                            expect_revision: input.revision,
                            label: "Batch Generate Shapes".to_string(),
                            undoable: true,
                            content: Box::new(doc),
                        }))
                    }),
                })
            },
        )
        .icon(icons::GENERATE)
        .enabled_when(|snap| snap.active_document.is_some()),
    );

    // Async scan (Phase 2: a lightweight async task on the shared executor; holds no thread
    // of its own)
    reg.register_command(
        CommandDef::async_command(
            "example.async_scan",
            "Async Scan (Executor)",
            |_ctx: &mut CommandCtx| {
                Ok(wb::AsyncSpec {
                    title: "Async Scan".to_string(),
                    cancellable: true,
                    input: Box::new(()),
                    job: Box::new(|task: wb::TaskCtx| {
                        Box::pin(async move {
                            let steps = 12u32;
                            for step in 0..steps {
                                task.check_cancelled()?;
                                task.report(
                                    Some(step as f32 / steps as f32),
                                    &format!("Scanning segment {}/{}", step + 1, steps),
                                );
                                // Platform-neutral wait (desktop async-io timer / web JS
                                // setTimeout)
                                wb::sleep(std::time::Duration::from_millis(100)).await;
                            }
                            task.log("Async scan finished (shared executor thread pool)");
                            Ok(None)
                        })
                    }),
                })
            },
        )
        .icon(icons::SCAN),
    );
    // Slow task (background, cancellable, no document commit)
    reg.register_command(
        CommandDef::background(
            "example.slow_job",
            "Slow Task (Progress and Cancel Demo)",
            |_ctx: &mut CommandCtx| {
                Ok(wb::TaskSpec {
                    title: "Slow Task".to_string(),
                    cancellable: true,
                    input: Box::new(()),
                    job: Box::new(|task: &mut wb::TaskCtx| {
                        let steps = 25u32;
                        for step in 0..steps {
                            task.check_cancelled()?;
                            task.report(
                                Some(step as f32 / steps as f32),
                                &format!("Step {}/{}", step + 1, steps),
                            );
                            if step == 10 {
                                task.log("Halfway done; try the Cancel button");
                            }
                            std::thread::sleep(std::time::Duration::from_millis(120));
                        }
                        Ok(None)
                    }),
                })
            },
        )
        .icon(icons::CLOCK),
    );

    // Failure demo
    reg.register_command(
        CommandDef::sync(
            "example.failing_cmd",
            "Failure Demo",
            |_ctx: &mut CommandCtx, _a: &CommandArgs| {
                CommandResult::failed(
                    "Demo: the command failed (see the Output panel and the status bar)",
                )
            },
        )
        .icon(icons::WARNING),
    );

    // Toggle the work mode
    reg.register_command(
        CommandDef::sync(
            "example.toggle_mode",
            "Toggle Work Mode",
            |ctx: &mut CommandCtx, _a: &CommandArgs| {
                let next = if ctx.snapshot().work_mode == "Modeling" {
                    "Sketch"
                } else {
                    "Modeling"
                };
                match ctx.set_work_mode(next) {
                    Ok(_) => CommandResult::done(),
                    Err(e) => CommandResult::failed(e),
                }
            },
        )
        .icon(icons::MODE)
        .checked_when(|snap| snap.work_mode == "Sketch"),
    );

    // Lineweight picker (Combo demo): combo_state_when supplies the options and the current
    // value; the chosen value comes back through args.value.
    static LINEWEIGHT_SELECTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    const LINEWEIGHTS: [&str; 4] = ["0.13 mm", "0.25 mm", "0.50 mm", "0.80 mm"];
    reg.register_command(
        CommandDef::sync(
            "example.set_lineweight",
            "Lineweight",
            |_ctx: &mut CommandCtx, a: &CommandArgs| {
                if let Some(v) = &a.value {
                    if let Some(i) = LINEWEIGHTS.iter().position(|o| o == v) {
                        LINEWEIGHT_SELECTED.store(i, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                CommandResult::done()
            },
        )
        .combo_state_when(|_snap| {
            Some(ComboState {
                current: LINEWEIGHTS[LINEWEIGHT_SELECTED.load(std::sync::atomic::Ordering::Relaxed)]
                    .to_string(),
                options: LINEWEIGHTS.iter().map(|s| s.to_string()).collect(),
            })
        }),
    );
}

fn register_toolbar(reg: &mut Registry) {
    // RibbonBar (design.md §7): the Home tab merges with the platform default tab; the Tools
    // tab holds the views and demos. The items also demonstrate the size tiers (Large/Small)
    // and the kinds (Command/Toggle/Split/Menu/Combo).
    reg.add_ribbon_tab(
        RibbonTab::new("app.home", "Home")
            .from_source("wb-example")
            .groups(vec![
                ToolbarGroup::new("example.file", "File")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::labeled("app.new_document", "New").large(),
                        ToolbarItem::labeled("app.open_document", "Open").large(),
                        ToolbarItem::labeled("app.save_document", "Save").large(),
                        ToolbarItem::command("app.import_document"),
                        ToolbarItem::command("app.export_document"),
                    ]),
                ToolbarGroup::new("example.edit", "Edit")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::command("app.undo"),
                        ToolbarItem::command("app.redo"),
                    ]),
                ToolbarGroup::new("example.shapes", "Shapes")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::command("example.add_shape").large(),
                        ToolbarItem::command("example.remove_selected"),
                        ToolbarItem::labeled(
                            "example.generate_shapes",
                            "Batch Generate (Background)",
                        ),
                    ]),
                ToolbarGroup::new("example.properties", "Attributes")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::labeled("example.set_lineweight", "Lineweight")
                            .combo()
                            .width(110.0),
                    ]),
            ]),
    );
    reg.add_ribbon_tab(
        RibbonTab::new("wb.example.tools", "Tools")
            .from_source("wb-example")
            .groups(vec![
                ToolbarGroup::new("example.view", "Views")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::labeled("example.toggle_mode", "Sketch Mode").toggle(),
                        ToolbarItem::labeled("example.open_canvas", "Canvas")
                            .split(vec![ToolbarItem::labeled(
                                "example.open_welcome",
                                "Welcome Page",
                            )]),
                        ToolbarItem::labeled("example.open_canvas", "View").menu(vec![
                            ToolbarItem::labeled("example.open_canvas", "Canvas"),
                            ToolbarItem::labeled("example.open_welcome", "Welcome Page"),
                        ]),
                    ]),
                ToolbarGroup::new("example.demo", "Demo")
                    .from_source("wb-example")
                    .items(vec![
                        ToolbarItem::labeled("example.async_scan", "Async Scan")
                            .split(vec![
                                ToolbarItem::labeled("example.slow_job", "Slow Task (Cancellable)"),
                                ToolbarItem::labeled("example.failing_cmd", "Failure Demo"),
                            ])
                            .large(),
                        ToolbarItem::labeled("example.slow_job", "Slow Task (Cancellable)"),
                        ToolbarItem::labeled("example.failing_cmd", "Failure Demo"),
                    ]),
            ]),
    );
}

/// Background task input (collected on the UI thread, consumed on the task thread).
#[derive(Clone)]
struct GenerateInput {
    doc_id: wb::DocumentId,
    revision: u64,
    base: ShapeDoc,
}
