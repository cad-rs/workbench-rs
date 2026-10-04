//! GUI 无关的绘制抽象（design.md §9：视图与渲染器分开建模）。
//!
//! 视图通过 [`PaintBackend`] 绘制内容，由选定的 GUI 前端（egui/gpui）提供具体实现。
//! 普通视图不需要 GPU；`wgpu` 能力将在后续里程碑作为并列的可选 trait 提供。

use crate::geo::{Color, Point, Rect, Size};

/// 视图/面板可用的最小 2D 绘制接口。坐标为视图本地区域内的点。
pub trait PaintBackend {
    /// 填充矩形（可带圆角）。
    fn fill_rect(&mut self, rect: Rect, color: Color, corner_radius: f32);
    /// 描边矩形。
    fn stroke_rect(&mut self, rect: Rect, color: Color, width: f32, corner_radius: f32);
    /// 画线段。
    fn line(&mut self, a: Point, b: Point, color: Color, width: f32);
    /// 填充圆。
    fn fill_circle(&mut self, center: Point, radius: f32, color: Color);
    /// 在 pos 处（文本左上角）绘制一行文本。
    fn text(&mut self, pos: Point, font_size: f32, color: Color, text: &str);
    /// 测量文本尺寸（供视图做命中测试/布局）。
    fn text_size(&mut self, font_size: f32, text: &str) -> Size;
    /// 绘制一个 SVG 图标（alpha 蒙版 + 指定颜色着色），在 rect 内等比居中。
    ///
    /// `path` 是资源路径（由产品的 AssetSource 解析）；资源缺失时前端不绘制
    /// （调用方可自行回退为文本）。默认实现为空，两个内置前端均已支持。
    fn draw_icon(&mut self, _rect: Rect, _path: &str, _color: Color) {}
}

/// 便捷绘制助手（按钮、进度条等通用小部件，供声明式面板与视图复用）。
pub struct PaintHelpers;

impl PaintHelpers {
    /// 绘制一个简单文本按钮，返回按钮矩形；调用方负责命中测试。
    pub fn button(
        p: &mut dyn PaintBackend,
        rect: Rect,
        label: &str,
        font_size: f32,
        enabled: bool,
        hovered: bool,
    ) -> Rect {
        let (bg, fg) = if !enabled {
            (Color::rgb(0.22, 0.22, 0.26), Color::GRAY)
        } else if hovered {
            (Color::ACCENT, Color::WHITE)
        } else {
            (Color::rgb(0.27, 0.29, 0.34), Color::WHITE)
        };
        let text_w = p.text_size(font_size, label).w;
        let w = (text_w + 16.0).max(rect.size.w);
        let r = Rect::new(rect.pos.x, rect.pos.y, w, rect.size.h);
        p.fill_rect(r, bg, 4.0);
        p.text(
            Point::new(r.pos.x + 8.0, r.pos.y + (r.size.h - font_size) * 0.5),
            font_size,
            fg,
            label,
        );
        r
    }

    /// 绘制一个带图标的按钮（图标在左、文本在右，整体在 rect 内居中）；
    /// 图标资源缺失时回退为纯文本按钮。返回按钮矩形；调用方负责命中测试。
    #[allow(clippy::too_many_arguments)]
    pub fn button_with_icon(
        p: &mut dyn PaintBackend,
        rect: Rect,
        icon: &str,
        label: &str,
        font_size: f32,
        enabled: bool,
        hovered: bool,
    ) -> Rect {
        let (bg, fg) = if !enabled {
            (Color::rgb(0.22, 0.22, 0.26), Color::GRAY)
        } else if hovered {
            (Color::ACCENT, Color::WHITE)
        } else {
            (Color::rgb(0.27, 0.29, 0.34), Color::WHITE)
        };
        let text_w = p.text_size(font_size, label).w;
        let icon_size = font_size * 1.15;
        let gap = if text_w > 0.0 { 6.0 } else { 0.0 };
        let content_w = icon_size + gap + text_w;
        let w = (content_w + 16.0).max(rect.size.w);
        let r = Rect::new(rect.pos.x, rect.pos.y, w, rect.size.h);
        p.fill_rect(r, bg, 4.0);
        let content_x = r.pos.x + (r.size.w - content_w) * 0.5;
        let icon_y = r.pos.y + (r.size.h - icon_size) * 0.5;
        p.draw_icon(
            Rect::new(content_x, icon_y, icon_size, icon_size),
            icon,
            fg,
        );
        p.text(
            Point::new(content_x + icon_size + gap, r.pos.y + (r.size.h - font_size) * 0.5),
            font_size,
            fg,
            label,
        );
        r
    }

    /// 绘制进度条。
    pub fn progress(p: &mut dyn PaintBackend, rect: Rect, fraction: Option<f32>, accent: Color) {
        p.fill_rect(rect, Color::rgb(0.2, 0.2, 0.24), 3.0);
        match fraction {
            Some(f) => {
                let f = f.clamp(0.0, 1.0);
                if f > 0.0 {
                    p.fill_rect(
                        Rect::new(rect.pos.x, rect.pos.y, rect.size.w * f, rect.size.h),
                        accent,
                        3.0,
                    );
                }
            }
            None => {
                // 不确定进度：来回滑块
                let t = (web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f32())
                    .unwrap_or(0.0)
                    * 2.0)
                    .sin();
                let f = 0.5 + 0.45 * t;
                p.fill_rect(
                    Rect::new(
                        rect.pos.x + rect.size.w * f * 0.6,
                        rect.pos.y,
                        rect.size.w * 0.3,
                        rect.size.h,
                    ),
                    accent,
                    3.0,
                );
            }
        }
    }
}
