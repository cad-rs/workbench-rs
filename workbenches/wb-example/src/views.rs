//! 示例 Workbench 的视图与面板：全部通过 GUI 无关的 PaintBackend 绘制，
//! 不依赖 egui / gpui（design.md 验收 #21）。

use workbench_api as wb;
use wb::{Color, PaintBackend, PaintHelpers, Point, Rect, ViewCtx};

use crate::shapes::ShapeDoc;

/// 示例扩展服务：跨面板/视图共享的选择状态（design.md §8.1 服务）。
#[derive(Default)]
pub struct ExampleState {
    /// (文档 ID, 选中图形索引)
    pub selected: Option<(wb::DocumentId, usize)>,
}

const CANVAS_VIEW: &str = "example.canvas";

// ---------------------------------------------------------------- 欢迎 ----

pub struct WelcomeView;

impl wb::ViewInstance for WelcomeView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.welcome")
    }
    fn title(&self) -> String {
        "欢迎".to_string()
    }
    fn paint(&mut self, p: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx) {
        p.fill_rect(rect, Color::rgb(0.15, 0.16, 0.19), 0.0);
        let x = rect.pos.x + 32.0;
        let mut y = rect.pos.y + 28.0;
        p.text(Point::new(x, y), 24.0, Color::WHITE, "workbench-rs 演示产品");
        y += 40.0;
        let snap = ctx.snapshot();
        let lines = [
            format!("工作模式：{}", snap.work_mode),
            match &snap.active_document {
                Some(doc) => format!(
                    "活动文档：{}{}（修订 r{}）",
                    doc.title,
                    if doc.dirty { " •未保存" } else { "" },
                    doc.revision
                ),
                None => "活动文档：（无）".to_string(),
            },
            format!("活动视图：{}", snap.active_view_type.as_deref().unwrap_or("（无）")),
            String::new(),
            "· 工具栏「图形」→ 添加图形：在画布中添加并撤销/重做".to_string(),
            "· 工具栏「演示」→ 批量生成：后台任务，可取消，完成后经文档版本校验提交".to_string(),
            "· 工具栏「演示」→ 慢任务：观察状态栏进度与协作式取消".to_string(),
            "· 画布中点击图形可选中，右侧属性/任务/输出面板联动".to_string(),
            "· Ctrl+S 保存 / Ctrl+Z 撤销 / Ctrl+W 关闭标签；布局自动保存恢复".to_string(),
        ];
        for line in lines {
            p.text(Point::new(x, y), 14.0, Color::LIGHT_GRAY, &line);
            y += 24.0;
        }
    }
}

// ---------------------------------------------------------------- 画布 ----

#[derive(Default)]
pub struct CanvasView;

impl wb::ViewInstance for CanvasView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new(CANVAS_VIEW)
    }
    fn title(&self) -> String {
        "图形画布".to_string()
    }

    fn paint(&mut self, p: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx) {
        p.fill_rect(rect, Color::rgb(0.17, 0.18, 0.22), 0.0);

        // 网格
        let step = 32.0;
        let mut gx = rect.pos.x + step;
        while gx < rect.right() {
            p.line(
                Point::new(gx, rect.pos.y),
                Point::new(gx, rect.bottom()),
                Color::rgba(1.0, 1.0, 1.0, 0.045),
                1.0,
            );
            gx += step;
        }
        let mut gy = rect.pos.y + step;
        while gy < rect.bottom() {
            p.line(
                Point::new(rect.pos.x, gy),
                Point::new(rect.right(), gy),
                Color::rgba(1.0, 1.0, 1.0, 0.045),
                1.0,
            );
            gy += step;
        }

        // 视图内联按钮：点击走命令系统（统一入口）
        let add_btn = Rect::new(rect.pos.x + 10.0, rect.pos.y + 10.0, 84.0, 26.0);
        let hover = ctx
            .input
            .mouse_pos
            .map(|m| add_btn.contains(m))
            .unwrap_or(false);
        let snap = ctx.snapshot();
        let add_enabled = snap.active_document.is_some() && snap.active_view_is(CANVAS_VIEW);
        PaintHelpers::button(p, add_btn, "+ 添加", 13.0, add_enabled, hover);

        // 图形
        let doc_id = snap.active_document.as_ref().map(|d| d.id);
        let selected = doc_id.and_then(|id| {
            ctx.host()
                .service::<ExampleState>()
                .and_then(|s| s.selected)
                .filter(|(d, _)| *d == id)
                .map(|(_, i)| i)
        });
        if let Some(doc) = doc_id {
            let shapes = ctx
                .host()
                .docs
                .read::<ShapeDoc, Vec<(f32, f32, f32, f32, bool, f32)>>(doc, |s| {
                    s.shapes
                        .iter()
                        .map(|sh| (sh.x, sh.y, sh.w, sh.h, sh.round, sh.hue))
                        .collect()
                })
                .unwrap_or_default();
            let origin = rect.pos;
            for (i, (x, y, w, h, round, hue)) in shapes.iter().enumerate() {
                let r = Rect::new(origin.x + x, origin.y + y, *w, *h);
                p.fill_rect(r, Color::from_hue(*hue), if *round { r.size.h * 0.5 } else { 4.0 });
                if selected == Some(i) {
                    p.stroke_rect(
                        r.translate(-2.0, -2.0).inflate(4.0),
                        Color::WHITE,
                        2.0,
                        if *round { (r.size.h + 8.0) * 0.5 } else { 6.0 },
                    );
                }
            }
        }

        // 交互：点击按钮 / 命中图形
        if ctx.input.clicked {
            if let Some(m) = ctx.input.mouse_pos {
                if add_enabled && add_btn.contains(m) {
                    ctx.run("example.add_shape");
                    return;
                }
                if let Some(doc) = doc_id {
                    let hits = ctx
                        .host()
                        .docs
                        .read::<ShapeDoc, Vec<Rect>>(doc, |s| {
                            s.shapes
                                .iter()
                                .map(|sh| {
                                    Rect::new(
                                        rect.pos.x + sh.x,
                                        rect.pos.y + sh.y,
                                        sh.w,
                                        sh.h,
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let hit = hits.iter().rposition(|r| r.contains(m));
                    let args = wb::CommandArgs::new().with(
                        "index",
                        hit.map(|i| i.to_string()).unwrap_or_default(),
                    );
                    let _ = ctx.run_with("example.select_shape", args);
                }
            }
        }
    }
}

// 小工具：Rect 外扩
trait Inflate {
    fn inflate(&self, by: f32) -> Rect;
}
impl Inflate for Rect {
    fn inflate(&self, by: f32) -> Rect {
        Rect::new(
            self.pos.x - by,
            self.pos.y - by,
            self.size.w + by * 2.0,
            self.size.h + by * 2.0,
        )
    }
}

// ---------------------------------------------------------------- 面板 ----

/// 左侧：文档与图形概览。
pub struct ExplorerPanel;

impl wb::ViewInstance for ExplorerPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.panel.explorer")
    }
    fn title(&self) -> String {
        "资源管理器".to_string()
    }
    fn paint(&mut self, p: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx) {
        p.fill_rect(rect, Color::rgb(0.16, 0.17, 0.20), 0.0);
        let mut y = rect.pos.y + 8.0;
        p.text(
            Point::new(rect.pos.x + 10.0, y),
            13.0,
            Color::ACCENT,
            "打开的文档",
        );
        y += 22.0;
        let docs: Vec<(String, bool, u64, usize)> = {
            let host = ctx.host();
            host.docs
                .docs()
                .map(|d| {
                    let count = host
                        .docs
                        .read::<ShapeDoc, usize>(d.id, |s| s.shapes.len())
                        .unwrap_or(0);
                    (d.title.clone(), d.dirty, d.revision, count)
                })
                .collect()
        };
        for (title, dirty, rev, count) in docs {
            let color = if dirty { Color::YELLOW } else { Color::LIGHT_GRAY };
            p.text(
                Point::new(rect.pos.x + 16.0, y),
                13.0,
                color,
                &format!("{title}{}", if dirty { " •" } else { "" }),
            );
            y += 18.0;
            p.text(
                Point::new(rect.pos.x + 24.0, y),
                11.0,
                Color::GRAY,
                &format!("图形 ×{count} · 修订 r{rev}"),
            );
            y += 22.0;
        }
        y += 8.0;
        let snap = ctx.snapshot();
        p.text(
            Point::new(rect.pos.x + 10.0, y),
            13.0,
            Color::ACCENT,
            "上下文",
        );
        y += 22.0;
        p.text(
            Point::new(rect.pos.x + 16.0, y),
            12.0,
            Color::LIGHT_GRAY,
            &format!("工作模式：{}", snap.work_mode),
        );
        y += 20.0;
        p.text(
            Point::new(rect.pos.x + 16.0, y),
            12.0,
            Color::LIGHT_GRAY,
            &format!(
                "活动视图：{}",
                snap.active_view_type.as_deref().unwrap_or("（无）")
            ),
        );
    }
}

/// 右侧：任务面板（进度条 + 取消按钮，点击命中由视图自管）。
pub struct TasksPanel;

impl wb::ViewInstance for TasksPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.panel.tasks")
    }
    fn title(&self) -> String {
        "任务".to_string()
    }
    fn paint(&mut self, p: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx) {
        p.fill_rect(rect, Color::rgb(0.16, 0.17, 0.20), 0.0);
        let tasks: Vec<wb::TaskInfo> = ctx.host().tasks.all().cloned().collect();
        if tasks.is_empty() {
            p.text(
                Point::new(rect.pos.x + 10.0, rect.pos.y + 10.0),
                13.0,
                Color::GRAY,
                "暂无后台任务。试试工具栏「演示」。",
            );
            return;
        }
        let mut y = rect.pos.y + 8.0;
        for task in tasks {
            let row_h = 64.0;
            let row = Rect::new(rect.pos.x + 4.0, y, rect.size.w - 8.0, row_h);
            p.fill_rect(row, Color::rgb(0.20, 0.21, 0.25), 4.0);
            let running = !task.status.is_terminal();
            let title_color = match task.status {
                wb::TaskStatus::Failed => Color::RED,
                wb::TaskStatus::Cancelled => Color::YELLOW,
                wb::TaskStatus::Succeeded => Color::GREEN,
                _ => Color::WHITE,
            };
            p.text(
                Point::new(row.pos.x + 8.0, y + 6.0),
                13.0,
                title_color,
                &format!("#{} {}", task.id.0, task.title),
            );
            p.text(
                Point::new(row.pos.x + 8.0, y + 24.0),
                11.0,
                Color::GRAY,
                &format!(
                    "[{}] {}{}",
                    task.kind.as_str(),
                    task.status.as_str(),
                    task.stage
                        .as_deref()
                        .map(|s| format!(" · {s}"))
                        .or_else(|| task.detail.clone().map(|d| format!(" · {d}")))
                        .unwrap_or_default()
                ),
            );
            if running {
                PaintHelpers::progress(
                    p,
                    Rect::new(row.pos.x + 8.0, y + 42.0, row.size.w - 64.0, 8.0),
                    task.progress,
                    Color::ACCENT,
                );
                // 取消按钮（自绘 + 命中测试）
                let btn = Rect::new(row.right() - 52.0, y + 34.0, 46.0, 22.0);
                let hover = ctx.input.mouse_pos.map(|m| btn.contains(m)).unwrap_or(false);
                PaintHelpers::button(p, btn, "取消", 11.0, true, hover);
                if ctx.input.clicked && ctx.input.mouse_pos.map(|m| btn.contains(m)).unwrap_or(false)
                {
                    ctx.host().tasks.request_cancel(task.id);
                }
            }
            y += row_h + 6.0;
        }
    }
}

/// 右侧：属性面板。
pub struct PropertiesPanel;

impl wb::ViewInstance for PropertiesPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.panel.properties")
    }
    fn title(&self) -> String {
        "属性".to_string()
    }
    fn paint(&mut self, p: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx) {
        p.fill_rect(rect, Color::rgb(0.16, 0.17, 0.20), 0.0);
        let snap = ctx.snapshot();
        let Some(doc) = snap.active_document.as_ref().map(|d| d.id) else {
            p.text(
                Point::new(rect.pos.x + 10.0, rect.pos.y + 10.0),
                13.0,
                Color::GRAY,
                "没有活动文档",
            );
            return;
        };
        let selected = ctx
            .host()
            .service::<ExampleState>()
            .and_then(|s| s.selected)
            .filter(|(d, _)| *d == doc)
            .map(|(_, i)| i);
        let shape = selected.and_then(|i| {
            ctx.host().docs.read::<ShapeDoc, (f32, f32, f32, f32, bool, f32)>(
                doc,
                |s| {
                    s.shapes
                        .get(i)
                        .map(|sh| (sh.x, sh.y, sh.w, sh.h, sh.round, sh.hue))
                        .unwrap_or_default()
                },
            )
        });
        let mut y = rect.pos.y + 10.0;
        match shape {
            Some((x, y0, w, h, round, hue)) => {
                p.text(
                    Point::new(rect.pos.x + 10.0, y),
                    13.0,
                    Color::ACCENT,
                    &format!("图形 #{}", selected.unwrap_or(0)),
                );
                y += 24.0;
                for (k, v) in [
                    ("X", format!("{x:.0}")),
                    ("Y", format!("{y0:.0}")),
                    ("宽", format!("{w:.0}")),
                    ("高", format!("{h:.0}")),
                    ("圆角", if round { "是" } else { "否" }.to_string()),
                    ("色相", format!("{hue:.0}°")),
                ] {
                    p.text(
                        Point::new(rect.pos.x + 16.0, y),
                        12.0,
                        Color::GRAY,
                        k,
                    );
                    p.text(
                        Point::new(rect.pos.x + 80.0, y),
                        12.0,
                        Color::LIGHT_GRAY,
                        &v,
                    );
                    y += 20.0;
                }
                y += 6.0;
                let swatch = Rect::new(rect.pos.x + 16.0, y, 48.0, 18.0);
                p.fill_rect(swatch, Color::from_hue(hue), 4.0);
            }
            None => {
                p.text(
                    Point::new(rect.pos.x + 10.0, y),
                    13.0,
                    Color::GRAY,
                    "在画布中点击图形以查看属性",
                );
            }
        }
    }
}

/// 底部：输出（平台日志）。
pub struct OutputPanel;

impl wb::ViewInstance for OutputPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.panel.output")
    }
    fn title(&self) -> String {
        "输出".to_string()
    }
    fn paint(&mut self, p: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx) {
        p.fill_rect(rect, Color::rgb(0.12, 0.125, 0.15), 0.0);
        let line_h = 17.0;
        let max_lines = ((rect.size.h - 8.0) / line_h) as usize;
        let entries = ctx.host().log.tail(max_lines.max(1));
        let mut y = rect.pos.y + 4.0;
        for e in entries {
            let color = match e.level {
                wb::LogLevel::Debug => Color::GRAY,
                wb::LogLevel::Info => Color::LIGHT_GRAY,
                wb::LogLevel::Warn => Color::YELLOW,
                wb::LogLevel::Error => Color::RED,
            };
            p.text(
                Point::new(rect.pos.x + 10.0, y),
                12.0,
                Color::GRAY,
                &e.time,
            );
            p.text(Point::new(rect.pos.x + 66.0, y), 12.0, color, &e.message);
            y += line_h;
        }
    }
}
