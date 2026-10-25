//! 通用视口宿主 Element（渲染器宿主契约）。
//!
//! 收编产品侧重复的整段壳代码（自 mulphy gpu3d.rs 迁入——两份壳曾各自
//! 漂移：0 高布局 bug 双份各修致「表面共享坏」误诊存续 7 轮、命中测试
//! 只有产品侧有；此后框架唯一实现）：满尺寸布局、指针/滚轮裸输入转发
//! （按下命中视口矩形才起拖拽）、渲染器与离屏合成目标槽、
//! `paint_surface`/readback 双路径呈现、无适配器深色底回退、超大视口
//! 护栏。
//!
//! 分层：渲染语义（相机灵敏度/几何/着色/线宽）归 [`workbench_render`]，
//! 本模块只递裸输入与目标参数；产品每帧构建 [`workbench_render::RenderScene`]
//! （世界坐标几何 + **基准**相机 + 背景）与基准视角 [`RenderViewportElement::base_view`]，
//! 交互相机偏移由渲染核心会话状态在 paint 时叠加。

use gpui::{Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, Pixels, Style, Window};
use workbench_render as wbr;

/// 通用渲染视口：产品递场景（基准相机）+ 基准视角，交互偏移由渲染核心
/// 叠加。Windows/Web 走 gpui 渲染器共享设备 `paint_surface` 零拷贝合成
/// （纹理全程留 GPU，尺寸不变即复用合成目标）；其余桌面 readback 位图
/// 贴图回退；无可用适配器深色底如实呈现。
pub struct RenderViewportElement {
    /// 每帧重建：世界坐标几何 + 基准相机（`scene_camera` 类语义）+ 背景。
    pub scene: wbr::RenderScene,
    /// `scene.camera` 对应的基准视角（yaw, pitch，度）——交互偏移叠加基准。
    pub base_view: (f64, f64),
}

/// 0xRRGGBB → gpui 颜色。
fn rgb(v: u32) -> gpui::Hsla {
    gpui::rgb_to_hsla(gpui::Rgba::new(
        ((v >> 16) & 0xFF) as f32 / 255.0,
        ((v >> 8) & 0xFF) as f32 / 255.0,
        (v & 0xFF) as f32 / 255.0,
        1.0,
    ))
}

// 渲染器单例（gpui UI 线程 thread_local；设备来自框架共享槽
// `shared_gpu`——Windows/Web 复用 gpui 渲染器自己的设备，其余桌面为
// 框架自建的独立设备）。
thread_local! {
    static RENDERER: std::cell::RefCell<Option<wbr::Renderer>> =
        const { std::cell::RefCell::new(None) };
}

// Windows/Web 的离屏合成目标（gpui 渲染器设备上的纹理；尺寸不变即复用
// ——纹理身份稳定，gpui SurfaceCache 的绑定缓存不重建）。
#[cfg(any(target_family = "wasm", target_os = "windows"))]
thread_local! {
    static SURFACE_TARGET:
        std::cell::RefCell<Option<(u32, u32, gpui_wgpu::WgpuRenderTarget)>> =
        const { std::cell::RefCell::new(None) };
}

/// 注册视口指针输入（每帧随 paint 重注册——gpui 每帧清监听）。
/// **壳只递裸参数**：拖拽位移（px）+ Shift 键位、滚轮像素 → 缩放乘子；
/// 灵敏度/换算/clamp 语义全在渲染核心（`wbr::CamOffset`）。按下时命中
/// 视口矩形才启动拖拽。
fn register_cam_input(window: &mut Window, bounds: Bounds<Pixels>) {
    use gpui::{MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent};
    let origin = bounds.origin;
    let size = bounds.size;
    let (ox, oy, sw, sh) = (
        f32::from(origin.x),
        f32::from(origin.y),
        f32::from(size.width),
        f32::from(size.height),
    );
    let in_bounds = move |p: gpui::Point<gpui::Pixels>| {
        let (px, py) = (f32::from(p.x), f32::from(p.y));
        px >= ox && py >= oy && px <= ox + sw && py <= oy + sh
    };
    thread_local! {
        static DRAGGING: std::cell::Cell<Option<(f32, f32)>> = const { std::cell::Cell::new(None) };
    }
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, _w, _cx| {
        if phase.bubble() && ev.button == gpui::MouseButton::Left && in_bounds(ev.position) {
            DRAGGING.with(|d| {
                d.set(Some((
                    f32::from(ev.position.x - origin.x),
                    f32::from(ev.position.y - origin.y),
                )))
            });
        }
    });
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _w, _cx| {
        if !phase.bubble() || !ev.dragging() {
            return;
        }
        DRAGGING.with(|d| {
            if let Some(last) = d.get() {
                let p = (
                    f32::from(ev.position.x - origin.x),
                    f32::from(ev.position.y - origin.y),
                );
                let (dx, dy) = ((p.0 - last.0) as f64, (p.1 - last.1) as f64);
                if dx != 0.0 || dy != 0.0 {
                    // 裸输入转发：Shift = 平移，默认 = 轨道（键位选择在壳，
                    // 位移语义在渲染核心）。
                    if ev.modifiers.shift {
                        wbr::cam_pan(dx, dy);
                    } else {
                        wbr::cam_orbit(dx, dy);
                    }
                    d.set(Some(p));
                }
            }
        });
    });
    window.on_mouse_event(move |_ev: &MouseUpEvent, phase, _w, _cx| {
        if phase.bubble() {
            DRAGGING.with(|d| d.set(None));
        }
    });
    window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, _w, _cx| {
        if !phase.bubble() || !in_bounds(ev.position) {
            return;
        }
        let delta = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(p) => p.y * 20.0,
        };
        wbr::cam_zoom((-delta as f64 * 0.0012).exp());
    });
}

impl RenderViewportElement {
    /// 无 GPU 适配器 / 渲染失败：深色底如实呈现（诊断走日志）。
    fn paint_fallback(window: &mut Window, bounds: Bounds<Pixels>) {
        let q = gpui::quad(
            bounds,
            gpui::Corners::all(gpui::px(0.)),
            rgb(0x1A1D24),
            gpui::Edges::all(gpui::px(0.)),
            gpui::transparent_black(),
            gpui::BorderStyle::default(),
        );
        window.paint_quad(q);
    }

    fn paint_scene(&mut self, window: &mut Window, bounds: Bounds<Pixels>) {
        // 交互相机输入（gpui 每帧清监听，随 paint 重注册——输入事件自动
        // 触发下一帧）。
        register_cam_input(window, bounds);
        let scale = window.scale_factor();
        let w = (f32::from(bounds.size.width) * scale).round().max(1.0) as u32;
        let h = (f32::from(bounds.size.height) * scale).round().max(1.0) as u32;
        // 上限防超大视口。
        let (w, h) = (w.min(4096), h.min(4096));
        // 基准相机（产品构建）→ 交互相机叠加（语义在渲染核心）。
        wbr::cam_apply(self.base_view.0, self.base_view.1, &mut self.scene.camera, h as f64);
        // 框架共享 GPU 槽（Windows/Web 复用 gpui 渲染器设备，其余桌面
        // 框架自建独立设备）；拿不到（无适配器）→ 深色底如实呈现。
        let Some(gpu) = super::shared_gpu(window) else {
            Self::paint_fallback(window, bounds);
            return;
        };
        RENDERER.with(|cell| {
            let mut slot = cell.borrow_mut();
            let renderer = slot.get_or_insert_with(|| {
                wbr::Renderer::new(gpu.device.clone(), gpu.queue.clone(), gpu.target_format)
            });
            #[cfg(any(target_family = "wasm", target_os = "windows"))]
            {
                // 渲进 gpui 渲染器设备上的离屏目标 → `paint_surface`
                // 零拷贝合成（纹理全程留 GPU；尺寸不变即复用目标）。
                SURFACE_TARGET.with(|t| {
                    let mut slot = t.borrow_mut();
                    let reuse =
                        slot.as_ref().is_some_and(|(tw, th, _)| *tw == w && *th == h);
                    if !reuse {
                        *slot = Some((
                            w,
                            h,
                            gpui_wgpu::WgpuRenderTarget::new(
                                gpu.context_handle(),
                                gpui::size(
                                    gpui::DevicePixels(w as i32),
                                    gpui::DevicePixels(h as i32),
                                ),
                            ),
                        ));
                    }
                    let target = &mut slot.as_mut().expect("just set").2;
                    renderer.render(&self.scene, w, h, target.view());
                    window.paint_surface(
                        bounds,
                        gpui::SurfaceSource::Texture {
                            texture: target.texture(),
                            size: target.size(),
                        },
                    );
                });
            }
            #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
            {
                // 其余桌面：gpui 渲染器非 wgpu，纹理无法合成——回读位图贴图
                // （headless 测试同路）。
                if let Some(bytes) = renderer.render_readback(&self.scene, w, h) {
                    let frame = image::Frame::new(
                        image::ImageBuffer::from_raw(w, h, bytes)
                            .expect("readback size matches target"),
                    );
                    let img = std::sync::Arc::new(gpui::RenderImage::new(vec![frame]));
                    let _ = window.paint_image(
                        bounds,
                        bounds,
                        gpui::Corners::all(gpui::px(0.)),
                        img,
                        0,
                        false,
                    );
                } else {
                    Self::paint_fallback(window, bounds);
                }
            }
        });
    }
}

impl gpui::IntoElement for RenderViewportElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for RenderViewportElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
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
        // 满尺寸：auto 布局在 flex 容器里会得到 0×0（paint 空转 → 黑屏）。
        let style = Style {
            size: gpui::Size::full(),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut gpui::App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        _cx: &mut gpui::App,
    ) {
        self.paint_scene(window, bounds);
    }
}
