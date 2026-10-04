//! SVG 图标光栅化与缓存（egui 前端）。
//!
//! 图标语义与 gpui 前端对齐：SVG 按 alpha 蒙版渲染，以调用方给定的颜色着色
//! （单色图标）；按 `(路径, 物理尺寸, 颜色)` 缓存纹理。资源缺失或解析失败时
//! 返回 `None`，调用方回退为纯文本展示。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{Color32, ColorImage, TextureHandle, TextureOptions, Vec2};

use workbench_api as wb;

/// SVG 图标缓存。
pub struct IconCache {
    assets: Arc<dyn wb::AssetSource>,
    textures: HashMap<IconKey, TextureHandle>,
    /// 解析失败过的路径（每条只诊断一次）。
    failed: HashSet<String>,
}

#[derive(PartialEq, Eq, Hash)]
struct IconKey {
    path: String,
    w: u32,
    h: u32,
    color: [u8; 4],
}

impl IconCache {
    pub fn new(assets: Arc<dyn wb::AssetSource>) -> Self {
        Self {
            assets,
            textures: HashMap::new(),
            failed: HashSet::new(),
        }
    }

    /// 渲染图标；`rect` 为期望区域（逻辑点），图标等比缩放并居中。
    /// 返回 `(纹理 ID, 实际绘制尺寸)`（实际尺寸按 SVG 宽高比可能小于 `rect`）。
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        path: &str,
        rect: Vec2,
        color: Color32,
    ) -> Option<(egui::TextureId, Vec2)> {
        let ppp = ctx.pixels_per_point();
        let box_w = (rect.x * ppp).round().max(1.0);
        let box_h = (rect.y * ppp).round().max(1.0);

        let bytes = self.assets.load(path)?;
        let tree = match resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default()) {
            Ok(t) => t,
            Err(e) => {
                if self.failed.insert(path.to_string()) {
                    eprintln!("wb-icon: SVG 解析失败 `{path}`: {e}");
                }
                return None;
            }
        };
        let svg = tree.size();
        let scale = (box_w / svg.width()).min(box_h / svg.height());
        let w = (svg.width() * scale).round().max(1.0) as u32;
        let h = (svg.height() * scale).round().max(1.0) as u32;
        let key = IconKey {
            path: path.to_string(),
            w,
            h,
            color: [color.r(), color.g(), color.b(), color.a()],
        };
        if let Some(t) = self.textures.get(&key) {
            return Some((t.id(), Vec2::new(w as f32 / ppp, h as f32 / ppp)));
        }

        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        // alpha 蒙版 × 着色 → 预乘 RGBA（tiny_skia 像素本身为预乘格式）
        let (tr, tg, tb, ta) = (
            color.r() as u32,
            color.g() as u32,
            color.b() as u32,
            color.a() as u32,
        );
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for px in pixmap.pixels() {
            let a = px.alpha() as u32;
            rgba.push((tr * a / 255) as u8);
            rgba.push((tg * a / 255) as u8);
            rgba.push((tb * a / 255) as u8);
            rgba.push((a * ta / 255) as u8);
        }
        let image = ColorImage::from_rgba_premultiplied([w as usize, h as usize], &rgba);
        let handle = ctx.load_texture(format!("icon:{path}"), image, TextureOptions::LINEAR);
        let id = handle.id();
        self.textures.insert(key, handle);
        Some((id, Vec2::new(w as f32 / ppp, h as f32 / ppp)))
    }
}
