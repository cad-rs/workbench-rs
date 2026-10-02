//! 平台内置面板（阶段 5，§18）：诊断面板与插件管理面板。
//! 通过 GUI 无关的 PaintBackend 绘制，随 platform_cmds 注册进每个产品。

use workbench_api as wb;
use wb::{Color, PaintBackend, PaintHelpers, Point, Rect, ViewCtx};

fn kv(p: &mut dyn PaintBackend, x: f32, y: f32, k: &str, v: &str) {
    p.text(Point::new(x, y), 12.0, Color::GRAY, k);
    p.text(Point::new(x + 110.0, y), 12.0, Color::LIGHT_GRAY, v);
}

// ---------------------------------------------------------------- 诊断 ----

pub struct DiagnosticsPanel;

impl wb::ViewInstance for DiagnosticsPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("app.panel.diagnostics")
    }
    fn title(&self) -> String {
        "诊断".to_string()
    }
    fn paint(&mut self, p: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx) {
        p.fill_rect(rect, Color::rgb(0.16, 0.17, 0.20), 0.0);
        let host = ctx.host();
        let mut y = rect.pos.y + 10.0;
        let x = rect.pos.x + 12.0;
        p.text(Point::new(x, y), 13.0, Color::ACCENT, "运行时");
        y += 24.0;
        kv(
            p,
            x,
            y,
            "产品",
            "（见窗口标题）",
        );
        y += 20.0;
        kv(
            p,
            x,
            y,
            "GUI 后端",
            &if host.stats.backend.is_empty() {
                "（未标注）".to_string()
            } else {
                host.stats.backend.clone()
            },
        );
        y += 20.0;
        kv(
            p,
            x,
            y,
            "运行时长",
            &format!("{:.1} 秒", host.uptime_secs()),
        );
        y += 20.0;
        kv(
            p,
            x,
            y,
            "帧统计",
            &format!(
                "{:.1} ms/帧（最近 {:.1}）× {}",
                host.stats.avg_frame_ms, host.stats.last_frame_ms, host.stats.frames
            ),
        );
        y += 26.0;
        p.text(Point::new(x, y), 13.0, Color::ACCENT, "文档与任务");
        y += 24.0;
        let docs: Vec<(String, bool, u64)> = {
            let h = ctx.host();
            h.docs
                .docs()
                .map(|d| (d.title.clone(), d.dirty, d.revision))
                .collect()
        };
        kv(p, x, y, "文档数", &format!("{}", docs.len()));
        y += 20.0;
        let dirty = docs.iter().filter(|(_, d, _)| *d).count();
        kv(p, x, y, "未保存", &format!("{dirty} 个"));
        y += 20.0;
        let (running, total) = {
            let h = ctx.host();
            (h.tasks.summary().0, h.tasks.all().count())
        };
        kv(p, x, y, "后台任务", &format!("运行 {running} / 累计 {total}"));
        y += 26.0;
        p.text(Point::new(x, y), 13.0, Color::ACCENT, "自动保存");
        y += 24.0;
        let interval = ctx.host().settings.get_f64("autosave.interval_secs", 30.0);
        kv(
            p,
            x,
            y,
            "间隔",
            &format!("{interval:.0} 秒（settings.json 可调）"),
        );
        y += 20.0;
        kv(
            p,
            x,
            y,
            "已自动保存",
            &format!("{} 次", ctx.host().stats.autosaves),
        );
    }
}


// ---------------------------------------------------------------- 插件 ----

pub struct PluginsPanel;

impl wb::ViewInstance for PluginsPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("app.panel.plugins")
    }
    fn title(&self) -> String {
        "插件".to_string()
    }
    fn paint(&mut self, p: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx) {
        p.fill_rect(rect, Color::rgb(0.16, 0.17, 0.20), 0.0);
        let plugins: Vec<wb::PluginEntry> = ctx.host().plugins.clone();
        if plugins.is_empty() {
            p.text(
                Point::new(rect.pos.x + 12.0, rect.pos.y + 10.0),
                13.0,
                Color::GRAY,
                "没有已注册的插件",
            );
            return;
        }
        let mut y = rect.pos.y + 10.0;
        let x = rect.pos.x + 12.0;
        for entry in &plugins {
            let (color, state_text) = match entry.state {
                "Loaded" => (Color::GREEN, "已装载"),
                "Disabled" => (Color::YELLOW, "已禁用"),
                _ => (Color::RED, "已拒绝"),
            };
            p.text(Point::new(x, y), 12.5, Color::WHITE, &entry.id);
            p.text(
                Point::new(x + rect.size.w * 0.55, y),
                12.0,
                color,
                state_text,
            );
            y += 20.0;
            if let Some(detail) = &entry.detail {
                p.text(
                    Point::new(x + 12.0, y),
                    11.0,
                    Color::GRAY,
                    &format!("· {detail}"),
                );
                y += 17.0;
            }
            // 禁用按钮（仅对已装载插件显示；命中测试由面板自管）
            if entry.state == "Loaded" {
                let btn = Rect::new(x + 12.0, y, 92.0, 22.0);
                let hover = ctx
                    .input
                    .mouse_pos
                    .map(|m| btn.contains(m))
                    .unwrap_or(false);
                PaintHelpers::button(p, btn, "禁用", 11.0, true, hover);
                if ctx.input.clicked
                    && ctx
                        .input
                        .mouse_pos
                        .map(|m| btn.contains(m))
                        .unwrap_or(false)
                {
                    let id = entry.id.clone();
                    let r = ctx.run_with(
                        "app.plugins.disable",
                        wb::CommandArgs::new().with("id", id),
                    );
                    let _ = r;
                }
                y += 28.0;
            }
            y += 6.0;
        }
    }
}
