//! 通用视口宿主 Element（渲染器宿主契约）。
//!
//! 收编产品侧重复的整段壳代码（自 mulphy gpu3d.rs 迁入——两份壳曾各自
//! 漂移：0 高布局 bug 双份各修致「表面共享坏」误诊存续 7 轮、命中测试
//! 只有产品侧有；此后框架唯一实现）。满尺寸布局、裸输入转发、上屏通路
//! 都在 [`super::present::Viewport`]；本模块只把输入映射到渲染核心的
//! 交互相机，并在 paint 时用渲染核心画产品的场景。无适配器时深色底如实
//! 呈现。
//!
//! 分层：渲染语义（相机灵敏度/几何/着色/线宽）归 [`workbench_render`]，
//! 本模块只递裸输入与目标参数；产品每帧构建 [`workbench_render::RenderScene`]
//! （世界坐标几何 + **基准**相机 + 背景）与基准视角 [`RenderViewportElement::base_view`]，
//! 交互相机偏移由渲染核心会话状态在渲染时叠加。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Bounds, Element, ElementId, GlobalElementId, HitboxBehavior, InspectorElementId, LayoutId,
    Pixels, Window,
};
use workbench_render as wbr;

use super::present::{Viewport, ViewportContent};

/// 通用渲染视口：产品递场景（基准相机）+ 基准视角，交互偏移由渲染核心
/// 叠加。上屏通路见 [`super::present`]；无可用适配器深色底如实呈现。
///
/// 进程内只有一个宿主（渲染器与上屏目标是单例，与迁移前相同）：同时
/// 显示两个本元素会争用同一目标。
pub struct RenderViewportElement {
    /// 每帧重建：世界坐标几何 + 基准相机（`scene_camera` 类语义）+ 背景。
    pub scene: wbr::RenderScene,
    /// `scene.camera` 对应的基准视角（yaw, pitch，度）——交互偏移叠加基准。
    pub base_view: (f64, f64),
}

/// The product's last scene and the renderer that draws it. The scene is
/// kept so a native surface can draw input-driven frames on its own, without
/// the product's view rendering again.
struct HostScene {
    renderer: Option<wbr::Renderer>,
    /// The scene, its base camera (before the interactive offset) and base
    /// view.
    scene: Option<(wbr::RenderScene, wbr::OrthoCamera, (f64, f64))>,
}

impl ViewportContent for HostScene {
    // 裸输入转发：拖拽位移（px）+ Shift 键位、滚轮像素 → 缩放乘子；灵敏度/
    // 换算/clamp 语义全在渲染核心（`wbr::CamOffset`）。
    fn drag(&mut self, dx: f32, dy: f32, shift: bool) -> bool {
        if shift {
            wbr::cam_pan(dx as f64, dy as f64);
        } else {
            wbr::cam_orbit(dx as f64, dy as f64);
        }
        true
    }

    fn scroll(&mut self, pixels: f32) -> bool {
        wbr::cam_zoom((-pixels as f64 * 0.0012).exp());
        true
    }

    fn draw(&mut self, view: &wgpu::TextureView, size: (u32, u32)) {
        let (Some(renderer), Some((scene, base_camera, base_view))) =
            (self.renderer.as_mut(), self.scene.as_mut())
        else {
            return;
        };
        // 基准相机（产品构建）→ 交互相机叠加（语义在渲染核心）。
        let mut camera = *base_camera;
        wbr::cam_apply(base_view.0, base_view.1, &mut camera, size.1 as f64);
        scene.camera = camera;
        renderer.render(scene, size.0, size.1, view);
    }
}

thread_local! {
    // gpui UI 线程单例；设备来自框架共享槽 `shared_gpu`。
    static HOST: Rc<RefCell<Viewport<HostScene>>> = Rc::new(RefCell::new(Viewport::new(
        HostScene {
            renderer: None,
            scene: None,
        },
    )));
}

impl gpui::IntoElement for RenderViewportElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for RenderViewportElement {
    type RequestLayoutState = ();
    type PrepaintState = gpui::Hitbox;

    fn id(&self) -> Option<ElementId> {
        // Element state keeps a native surface's liveness token.
        Some("render-viewport".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> (LayoutId, ()) {
        (super::present::full_size_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> gpui::Hitbox {
        let host = HOST.with(Rc::clone);
        // 框架共享 GPU 槽；拿不到（无适配器）→ paint 深色底如实呈现。
        let gpu = super::shared_gpu(window);
        {
            let mut viewport = host.borrow_mut();
            // The product rebuilt its scene for this frame.
            let scene = std::mem::take(&mut self.scene);
            let base_camera = scene.camera;
            viewport.content.scene = Some((scene, base_camera, self.base_view));
            if let Some(gpu) = &gpu {
                viewport.content.renderer.get_or_insert_with(|| {
                    wbr::Renderer::new(gpu.device.clone(), gpu.queue.clone(), gpu.target_format)
                });
            }
            viewport.invalidate();
        }
        match gpu {
            Some(gpu) => Viewport::prepaint(&host, &gpu, bounds, window, cx),
            None => window.insert_hitbox(bounds, HitboxBehavior::Normal),
        }
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        hitbox: &mut gpui::Hitbox,
        window: &mut Window,
        _cx: &mut gpui::App,
    ) {
        let host = HOST.with(Rc::clone);
        let id = id.expect("the element has an id");
        if !Viewport::paint(&host, id, bounds, hitbox, window) {
            // 无 GPU 适配器 / 渲染失败：深色底如实呈现（诊断走日志）。
            window.paint_quad(gpui::fill(bounds, gpui::rgb(0x1A1D24)));
        }
    }
}
