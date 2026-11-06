//! 通用视口宿主 Element（渲染器宿主契约）。
//!
//! 收编产品侧重复的整段壳代码（自 mulphy gpu3d.rs 迁入——两份壳曾各自
//! 漂移：0 高布局 bug 双份各修致「表面共享坏」误诊存续 7 轮、命中测试
//! 只有产品侧有；此后框架唯一实现）：满尺寸布局、指针/滚轮裸输入转发
//! （按下命中视口矩形才起拖拽）、渲染器与上屏通路（[`super::present`]：
//! Web 动态纹理 / Windows 原生合成表面 / 其余回读贴图）、无适配器深色底
//! 回退、超大视口护栏。
//!
//! 分层：渲染语义（相机灵敏度/几何/着色/线宽）归 [`workbench_render`]，
//! 本模块只递裸输入与目标参数；产品每帧构建 [`workbench_render::RenderScene`]
//! （世界坐标几何 + **基准**相机 + 背景）与基准视角 [`RenderViewportElement::base_view`]，
//! 交互相机偏移由渲染核心会话状态在渲染时叠加。

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gpui::{
    Bounds, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId,
    LayoutId, Pixels, Point, Style, Window,
};
use workbench_render as wbr;

use super::present::{NativeHooks, PointerInput, Presenter};
use super::Gpu;

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

/// 0xRRGGBB → gpui 颜色。
fn rgb(v: u32) -> gpui::Hsla {
    gpui::rgb(v).into()
}

/// The host singleton: renderer, presenter and the product's last scene.
/// The scene is kept so a native surface can draw input-driven frames on its
/// own, without the product's view rendering again.
struct Host {
    gpu: Option<Rc<Gpu>>,
    renderer: Option<wbr::Renderer>,
    presenter: Presenter,
    /// The product's last scene and its base camera (before the interactive
    /// offset is applied).
    scene: Option<(wbr::RenderScene, wbr::OrthoCamera, (f64, f64))>,
    dirty: bool,
    /// Where a left-button drag over the viewport last was (viewport-local).
    drag_from: Option<(f32, f32)>,
    /// Where the element was last laid out (window logical pixels).
    origin: Point<Pixels>,
}

thread_local! {
    // gpui UI 线程单例；设备来自框架共享槽 `shared_gpu`。
    static HOST: Rc<RefCell<Host>> = Rc::new(RefCell::new(Host {
        gpu: None,
        renderer: None,
        presenter: Presenter::Pending,
        scene: None,
        dirty: true,
        drag_from: None,
        origin: Point::default(),
    }));
}

impl Host {
    /// 裸输入转发：拖拽位移（px）+ Shift 键位、滚轮像素 → 缩放乘子；
    /// 灵敏度/换算/clamp 语义全在渲染核心（`wbr::CamOffset`）。
    fn pointer(&mut self, input: PointerInput) -> bool {
        let local = |p: Point<Pixels>, origin: Point<Pixels>| {
            (f32::from(p.x - origin.x), f32::from(p.y - origin.y))
        };
        let changed = match input {
            PointerInput::Press(position) => {
                self.drag_from = Some(local(position, self.origin));
                false
            }
            PointerInput::Release => {
                self.drag_from = None;
                false
            }
            PointerInput::Move {
                position,
                dragging,
                shift,
            } => match self.drag_from {
                Some(_) if !dragging => {
                    self.drag_from = None;
                    false
                }
                Some(from) => {
                    let p = local(position, self.origin);
                    let (dx, dy) = ((p.0 - from.0) as f64, (p.1 - from.1) as f64);
                    self.drag_from = Some(p);
                    if dx == 0.0 && dy == 0.0 {
                        false
                    } else {
                        // 键位选择在壳（Shift = 平移，默认 = 轨道），位移语义在
                        // 渲染核心。
                        if shift {
                            wbr::cam_pan(dx, dy);
                        } else {
                            wbr::cam_orbit(dx, dy);
                        }
                        true
                    }
                }
                None => false,
            },
            PointerInput::Scroll(pixels) => {
                wbr::cam_zoom((-pixels as f64 * 0.0012).exp());
                true
            }
        };
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// Renders the last scene through `render` (paint or a native redraw),
    /// with the interactive camera offset as it is now.
    fn render_with(
        &mut self,
        render: impl FnOnce(&mut Presenter, &Gpu, &mut dyn FnMut(&wgpu::TextureView, (u32, u32))),
    ) {
        let Some(gpu) = self.gpu.clone() else {
            return;
        };
        let Self {
            renderer,
            presenter,
            scene,
            ..
        } = self;
        let (Some(renderer), Some((scene, base_camera, base_view))) = (renderer.as_mut(), scene.as_mut())
        else {
            return;
        };
        let mut draw = |view: &wgpu::TextureView, size: (u32, u32)| {
            // 基准相机（产品构建）→ 交互相机叠加（语义在渲染核心）。
            let mut camera = *base_camera;
            wbr::cam_apply(base_view.0, base_view.1, &mut camera, size.1 as f64);
            scene.camera = camera;
            renderer.render(scene, size.0, size.1, view);
        };
        render(presenter, &gpu, &mut draw);
        self.dirty = false;
    }
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
}

impl gpui::IntoElement for RenderViewportElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for RenderViewportElement {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

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
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Hitbox {
        let host = HOST.with(Rc::clone);
        let weak = Rc::downgrade(&host);
        let mut state = host.borrow_mut();
        state.origin = bounds.origin;
        // The product rebuilt its scene for this frame: keep it (and its base
        // camera) for the frames the viewport draws on its own.
        let scene = std::mem::take(&mut self.scene);
        let base_camera = scene.camera;
        state.scene = Some((scene, base_camera, self.base_view));
        state.dirty = true;
        // 框架共享 GPU 槽；拿不到（无适配器）→ paint 深色底如实呈现。
        if let Some(gpu) = super::shared_gpu(window) {
            if state.renderer.is_none() {
                state.renderer = Some(wbr::Renderer::new(
                    gpu.device.clone(),
                    gpu.queue.clone(),
                    gpu.target_format,
                ));
            }
            state.gpu = Some(gpu.clone());
            state
                .presenter
                .prepaint(&gpu, bounds, window, cx, || native_hooks(weak));
        }
        drop(state);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        _cx: &mut gpui::App,
    ) {
        let host = HOST.with(Rc::clone);
        let native_input = {
            let mut state = host.borrow_mut();
            if state.gpu.is_none() || matches!(state.presenter, Presenter::Pending) {
                drop(state);
                Self::paint_fallback(window, bounds);
                return;
            }
            let dirty = state.dirty;
            let mut result = Ok(());
            state.render_with(|presenter, gpu, draw| {
                result = presenter.paint(gpu, bounds, dirty, window, draw);
            });
            if let Err(err) = result {
                eprintln!("render viewport: {err:#}");
                drop(state);
                Self::paint_fallback(window, bounds);
                return;
            }
            state.presenter.native_input()
        };
        if !native_input {
            // 交互相机输入（gpui 每帧清监听，随 paint 重注册）。
            let view = window.current_view();
            super::present::listen_pointer(
                window,
                hitbox,
                Rc::new(move |input| host.borrow_mut().pointer(input)),
                view,
            );
        }
    }
}

/// How the host's native surface draws and takes input on its own.
fn native_hooks(host: Weak<RefCell<Host>>) -> NativeHooks {
    let redraw = host.clone();
    NativeHooks {
        redraw: Box::new(move || {
            if let Some(host) = redraw.upgrade() {
                host.borrow_mut()
                    .render_with(|presenter, _gpu, draw| presenter.redraw_native(draw));
            }
        }),
        pointer: Rc::new(move |input| {
            host.upgrade()
                .is_some_and(|host| host.borrow_mut().pointer(input))
        }),
    }
}
