//! GPU 视口（前端原生视图扩展）：自建 wgpu 管线渲染 3D/2D 视图。
//!
//! 帧怎么上屏由 [`present`] 按平台决定：Web 在 GPUI 自己的设备上渲进
//! `WgpuDynamicTexture`（按 sprite 参与绘制顺序）；Windows 在 GPUI 窗口合成
//! 里拿一个原生表面（DirectComposition visual），挂视口自己的 DX12 交换链
//! （位于 GPUI 基础内容之上、菜单/弹层/对话框之下）；其余桌面或合成不可用
//! 时离屏渲染 + 回读贴图。移植自 gpui-wgpu 参考实现（javagg/gpui-demo），
//! 上屏方式依 javagg/gpui-fast-wgpu-demo。
//!
//! 集成方式：[`GpuViewportView`] 实现 [`wb::ViewInstance`]，其 `element`
//! 返回一个 [`GpuViewportElement`]——一个自绘的 gpui `Element`，在 prepaint
//! 定位、paint 渲染。
//!
//! 设备不归属任何一个视图：[`shared_gpu`] 提供全进程共享的 [`Gpu`]，
//! 供插件自带的自定义 gpui `Element` 建自己的渲染资源。

mod camera;
mod gpu;
mod present;
// 产品注入契约（设计文档「gpu 视口注入 API」）：mesh/scene 公开，产品可
// 自建着色网格（Scene::from_mesh）并经 GpuViewportView::new3d_with_scene
// 注入轨道视口；camera 仍由框架驱动（Orbit3d 交互复用）。
pub mod mesh;
pub mod scene;
// 通用视口宿主 Element（workbench-render 渲染核心的壳：裸输入转发 +
// 按平台上屏；产品递 RenderScene + 基准视角）。
pub mod host;
pub use host::RenderViewportElement;

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use camera::{OrbitCamera, OrthoCamera};
use gpui::{
    App, AnyElement, Bounds, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, Style, Window,
};
use present::{NativeHooks, PointerInput, Presenter};
use scene::{Pipelines, Scene, Uniforms, ViewState};
use workbench_api as wb;

pub use gpu::{Gpu, GpuRequirements};
pub(crate) use gpu::set_requirements;

#[derive(Clone, Copy)]
enum ViewKind {
    /// 3D 轨道视图：左键拖拽旋转，Shift+左键平移，滚轮缩放
    Orbit3d,
    /// 俯视正交视图：左键拖拽平移，滚轮缩放
    Top2d,
}

/// Shared state of the demo GPU viewports (scene + pipelines).
///
/// The device is deliberately *not* owned here: it is the process-wide
/// [`Gpu`] that every GPU view renders on. Only this module's own
/// scene and pipeline set live here.
struct ViewportCommon {
    gpu: Rc<Gpu>,
    scene: Rc<Scene>,
    pipes: Rc<Pipelines>,
}

impl ViewportCommon {
    fn new(gpu: Rc<Gpu>) -> Self {
        let scene = Rc::new(Scene::new(&gpu.device));
        let pipes = Rc::new(Pipelines::new(&gpu.device, gpu.target_format));
        Self { gpu, scene, pipes }
    }
}

/// Shared GPU slot (rendering happens on the gpui thread).
enum GpuSlot {
    /// Not created yet (web: graphics not up yet, retried on the next frame).
    NotTried,
    Ready(Rc<Gpu>),
    /// Device creation failed; not retried, so it does not block every frame.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    Failed,
}

thread_local! {
    static SHARED_GPU: RefCell<GpuSlot> = const { RefCell::new(GpuSlot::NotTried) };
}

#[cfg(target_family = "wasm")]
thread_local! {
    /// The web platform the application runs on: it holds GPUI's wgpu
    /// context, which the viewports render on.
    static WEB_PLATFORM: RefCell<Option<Rc<gpui_web::WebPlatform>>> = const { RefCell::new(None) };
}

/// Web: remember the platform, whose wgpu context [`shared_gpu`] hands out.
#[cfg(target_family = "wasm")]
pub(crate) fn set_web_platform(platform: Rc<gpui_web::WebPlatform>) {
    WEB_PLATFORM.with(|slot| *slot.borrow_mut() = Some(platform));
}

/// Acquire the process-wide GPU context, creating it on first use.
///
/// A plugin-provided GPU view calls this from its `Element` and builds its own
/// pipelines, textures and render targets on the returned device. On the web
/// this is GPUI's own device (so its textures can be sampled by GPUI); on the
/// desktop it is a device of the viewports' own.
///
/// Returns `None` until the device is available (web: until graphics are up)
/// or when no adapter exists; callers paint a placeholder and retry on a
/// later frame.
pub fn shared_gpu(_window: &Window) -> Option<Rc<Gpu>> {
    SHARED_GPU.with(|cell| {
        let mut slot = cell.borrow_mut();
        if matches!(&*slot, GpuSlot::NotTried) {
            #[cfg(target_family = "wasm")]
            {
                let context = WEB_PLATFORM
                    .with(|platform| platform.borrow().as_ref().and_then(|p| p.wgpu_context()));
                if let Some(context) = context {
                    *slot = GpuSlot::Ready(Rc::new(Gpu::from_web_context(context)));
                }
            }
            #[cfg(not(target_family = "wasm"))]
            {
                match Gpu::new() {
                    Ok(gpu) => *slot = GpuSlot::Ready(Rc::new(gpu)),
                    Err(err) => {
                        eprintln!("gpu viewport: failed to init wgpu device: {err:#}");
                        *slot = GpuSlot::Failed;
                    }
                }
            }
        }
        match &*slot {
            GpuSlot::Ready(gpu) => Some(gpu.clone()),
            _ => None,
        }
    })
}

// Demo viewport state (scene + pipelines), built once on top of the shared
// device and reused by every demo viewport in the process.
thread_local! {
    static DEMO_VIEWPORT: RefCell<Option<Rc<ViewportCommon>>> = const { RefCell::new(None) };
}

fn demo_viewport(window: &Window) -> Option<Rc<ViewportCommon>> {
    if let Some(common) = DEMO_VIEWPORT.with(|cell| cell.borrow().as_ref().cloned()) {
        return Some(common);
    }
    let common = Rc::new(ViewportCommon::new(shared_gpu(window)?));
    DEMO_VIEWPORT.with(|cell| *cell.borrow_mut() = Some(common.clone()));
    Some(common)
}

/// A depth buffer sized to the frame (the presenters only hand out color).
struct DepthTarget {
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl DepthTarget {
    fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport-depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Depth24Plus,
            view_formats: &[],
        });
        Self {
            view: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            size,
        }
    }
}

fn uniforms_for(kind: ViewKind, orbit: &OrbitCamera, ortho: &OrthoCamera, aspect: f32) -> Uniforms {
    match kind {
        ViewKind::Orbit3d => Uniforms::new(
            orbit.view_proj(aspect),
            orbit.eye(),
            glam::Vec3::new(0.45, 0.8, 0.35).normalize(),
            0.002,
        ),
        ViewKind::Top2d => Uniforms::new(
            ortho.view_proj(aspect),
            glam::Vec3::new(ortho.center.x, 1000.0, ortho.center.z),
            // Light straight down the view direction: an elevation-drawing
            // style flat projection.
            glam::Vec3::new(0.0, -1.0, 0.0),
            0.002,
        ),
    }
}

/// Encodes and submits one frame of `common`'s scene into `view`.
fn draw_frame(
    common: &ViewportCommon,
    view_state: &ViewState,
    depth: &mut Option<DepthTarget>,
    uniforms: &Uniforms,
    view: &wgpu::TextureView,
    size: (u32, u32),
) {
    let gpu = &common.gpu;
    if depth.as_ref().is_none_or(|d| d.size != size) {
        *depth = Some(DepthTarget::new(&gpu.device, size));
    }
    let depth = depth.as_ref().expect("allocated above");
    gpu.queue
        .write_buffer(&view_state.uniform_buf, 0, bytemuck::bytes_of(uniforms));
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viewport-encoder"),
        });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewport-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.102,
                        g: 0.110,
                        b: 0.133,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        scene::draw_scene(&mut pass, &common.scene, &common.pipes, &view_state.bind_group);
    }
    gpu.queue.submit([encoder.finish()]);
}

/// Mutable viewport state, shared between the view and its element.
///
/// The element needs live access to the camera and the GPU resources, so the
/// state lives behind an `Rc` that both sides clone.
struct GpuViewportInner {
    kind: ViewKind,
    orbit: OrbitCamera,
    ortho: OrthoCamera,
    view_state: Option<ViewState>,
    depth: Option<DepthTarget>,
    presenter: Presenter,
    /// What the viewport draws: the demo scene or the product's injected one.
    common: Option<Rc<ViewportCommon>>,
    /// The camera changed since the last frame.
    dirty: bool,
    /// A left-button drag started over the viewport (pointer position of the
    /// previous event, viewport-local logical pixels).
    drag_from: Option<(f32, f32)>,
    /// Where the element was last laid out (window logical pixels).
    origin: Point<Pixels>,
    /// 注入场景的每视图缓存：(provider, common)——provider 指针同一性
    /// 变化（产品换了场景源）即重建；demo 视口不经此路径。
    injected: Option<(Rc<SceneProvider>, Rc<ViewportCommon>)>,
}

impl GpuViewportInner {
    fn new(kind: ViewKind) -> Self {
        Self {
            kind,
            orbit: OrbitCamera::new(),
            ortho: OrthoCamera::new(),
            view_state: None,
            depth: None,
            presenter: Presenter::Pending,
            common: None,
            dirty: true,
            drag_from: None,
            origin: Point::default(),
            injected: None,
        }
    }

    /// The scene this viewport draws, built on first use. `None` until the
    /// device is available (or the product's provider has no scene yet).
    fn resolve_common(
        &mut self,
        provider: Option<&Rc<SceneProvider>>,
        window: &Window,
    ) -> Option<Rc<ViewportCommon>> {
        let common = match provider {
            Some(provider) => {
                let cached = self
                    .injected
                    .as_ref()
                    .is_some_and(|(p, _)| Rc::ptr_eq(p, provider));
                if !cached {
                    let gpu = shared_gpu(window)?;
                    let scene = provider(&gpu.device)?;
                    let pipes = Rc::new(Pipelines::new(&gpu.device, gpu.target_format));
                    self.injected = Some((
                        provider.clone(),
                        Rc::new(ViewportCommon { gpu, scene, pipes }),
                    ));
                    self.dirty = true;
                }
                self.injected.as_ref().map(|(_, c)| c.clone())?
            }
            None => demo_viewport(window)?,
        };
        if self.view_state.is_none() {
            self.view_state = Some(ViewState::new(&common.gpu.device, &common.pipes.layout));
        }
        self.common = Some(common.clone());
        Some(common)
    }

    /// Applies pointer input to the camera; true when the view changed.
    fn pointer(&mut self, input: PointerInput) -> bool {
        let local = |p: Point<Pixels>, origin: Point<Pixels>| {
            (f32::from(p.x - origin.x), f32::from(p.y - origin.y))
        };
        match input {
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
            } => {
                let Some(from) = self.drag_from else {
                    return false;
                };
                if !dragging {
                    // The release happened elsewhere.
                    self.drag_from = None;
                    return false;
                }
                let p = local(position, self.origin);
                let (dx, dy) = (p.0 - from.0, p.1 - from.1);
                self.drag_from = Some(p);
                if dx == 0.0 && dy == 0.0 {
                    return false;
                }
                match (self.kind, shift) {
                    (ViewKind::Orbit3d, false) => self.orbit.orbit(dx, dy),
                    (ViewKind::Orbit3d, true) => self.orbit.pan(dx, dy),
                    (ViewKind::Top2d, _) => self.ortho.pan(dx, dy),
                }
                self.dirty = true;
                true
            }
            PointerInput::Scroll(pixels) => {
                let factor = (-pixels * 0.0012).exp();
                match self.kind {
                    ViewKind::Orbit3d => self.orbit.zoom(factor),
                    ViewKind::Top2d => self.ortho.zoom(factor),
                }
                self.dirty = true;
                true
            }
        }
    }

    /// Renders a frame through `presenter`'s `render` (paint or a native
    /// redraw), from the camera as it is now.
    fn render_with(
        &mut self,
        render: impl FnOnce(&mut Presenter, &mut dyn FnMut(&wgpu::TextureView, (u32, u32))),
    ) {
        let Some(common) = self.common.clone() else {
            return;
        };
        let Self {
            kind,
            orbit,
            ortho,
            view_state,
            depth,
            presenter,
            ..
        } = self;
        let Some(view_state) = view_state.as_ref() else {
            return;
        };
        let mut draw = |view: &wgpu::TextureView, size: (u32, u32)| {
            let uniforms = uniforms_for(*kind, orbit, ortho, size.0 as f32 / size.1 as f32);
            draw_frame(&common, view_state, depth, &uniforms, view, size);
        };
        render(presenter, &mut draw);
        self.dirty = false;
    }

    /// Native surfaces: a frame outside GPUI's draw, after input.
    fn redraw_native(&mut self) {
        self.render_with(|presenter, draw| presenter.redraw_native(draw));
    }

    fn prepaint(
        &mut self,
        this: Weak<RefCell<Self>>,
        provider: Option<&Rc<SceneProvider>>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.origin = bounds.origin;
        let Some(common) = self.resolve_common(provider, window) else {
            return;
        };
        self.presenter.prepaint(&common.gpu, bounds, window, cx, || {
            let redraw = this.clone();
            NativeHooks {
                redraw: Box::new(move || {
                    if let Some(inner) = redraw.upgrade() {
                        inner.borrow_mut().redraw_native();
                    }
                }),
                pointer: Rc::new(move |input| {
                    this.upgrade()
                        .is_some_and(|inner| inner.borrow_mut().pointer(input))
                }),
            }
        });
    }

    /// Draw one frame into the view area, or a placeholder background until
    /// the GPU context (or the injected scene) is available.
    fn paint(&mut self, window: &mut Window, bounds: Bounds<Pixels>) {
        let Some(common) = self.common.clone() else {
            crate::paint_placeholder(window, bounds);
            return;
        };
        if matches!(self.presenter, Presenter::Pending) {
            crate::paint_placeholder(window, bounds);
            return;
        }
        let dirty = self.dirty;
        let Self {
            kind,
            orbit,
            ortho,
            view_state,
            depth,
            presenter,
            ..
        } = self;
        let Some(view_state) = view_state.as_ref() else {
            crate::paint_placeholder(window, bounds);
            return;
        };
        let mut draw = |view: &wgpu::TextureView, size: (u32, u32)| {
            let uniforms = uniforms_for(*kind, orbit, ortho, size.0 as f32 / size.1 as f32);
            draw_frame(&common, view_state, depth, &uniforms, view, size);
        };
        if let Err(err) = presenter.paint(&common.gpu, bounds, dirty, window, &mut draw) {
            eprintln!("gpu viewport render error: {err:#}");
            crate::paint_placeholder(window, bounds);
        }
        self.dirty = false;
    }
}

/// 产品场景提供者：每次 paint 调用，返回注入场景（`None` → 占位符）。
/// 实现方自缓存（按内容身份判定重建），返回 `Rc` 保证与注入缓存指针
/// 同一性对比成立（避免每帧重建上传缓冲）。
pub type SceneProvider = dyn Fn(&wgpu::Device) -> Option<Rc<Scene>>;

/// A GPU viewport view instance (central tab content).
pub struct GpuViewportView {
    type_id: &'static str,
    title: &'static str,
    inner: Rc<RefCell<GpuViewportInner>>,
    scene_provider: Option<Rc<SceneProvider>>,
}

impl GpuViewportView {
    fn new3d() -> Self {
        Self::new(ViewKind::Orbit3d, "example.gpu3d", "三维视图 (wgpu)")
    }

    fn new_top() -> Self {
        Self::new(ViewKind::Top2d, "example.gpu_top", "俯视图 (wgpu)")
    }

    fn new(kind: ViewKind, type_id: &'static str, title: &'static str) -> Self {
        Self {
            type_id,
            title,
            inner: Rc::new(RefCell::new(GpuViewportInner::new(kind))),
            scene_provider: None,
        }
    }

    /// 产品注入场景的 3D 轨道视口（注入 API）：`type_id`/`title` 由产品
    /// 指定；`scene_provider` 每次 paint 调用（实现方自缓存），`None` 时
    /// 显示占位符。相机交互（轨道/平移/缩放）复用框架实现。
    pub fn new3d_with_scene(
        type_id: &'static str,
        title: &'static str,
        scene_provider: Rc<SceneProvider>,
    ) -> Self {
        Self {
            type_id,
            title,
            inner: Rc::new(RefCell::new(GpuViewportInner::new(ViewKind::Orbit3d))),
            scene_provider: Some(scene_provider),
        }
    }
}

impl wb::ViewInstance for GpuViewportView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new(self.type_id)
    }

    fn title(&self) -> String {
        self.title.to_string()
    }

    fn element(&mut self, _host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let element: AnyElement = GpuViewportElement {
            inner: self.inner.clone(),
            scene_provider: self.scene_provider.clone(),
        }
        .into_any_element();
        Some(Box::new(element))
    }
}

/// A gpui element that renders one GPU viewport.
///
/// It is placed in prepaint and draws in paint. Pointer input reaches the
/// camera either through GPUI listeners (registered in paint, as GPUI clears
/// them every frame) or, over a native surface, through the surface's own
/// input handler.
struct GpuViewportElement {
    inner: Rc<RefCell<GpuViewportInner>>,
    scene_provider: Option<Rc<SceneProvider>>,
}

impl IntoElement for GpuViewportElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for GpuViewportElement {
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
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // 自绘 Element 必须显式满尺寸：auto 尺寸在 flex 容器里得 0 高，
        // paint 空转（实测视口 900×1 全黑；同 mulphy round-182 e070835 教训）。
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
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let this = Rc::downgrade(&self.inner);
        self.inner.borrow_mut().prepaint(
            this,
            self.scene_provider.as_ref(),
            bounds,
            window,
            cx,
        );
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let native_input = {
            let mut inner = self.inner.borrow_mut();
            inner.paint(window, bounds);
            inner.presenter.native_input()
        };
        if !native_input {
            let inner = self.inner.clone();
            let view = window.current_view();
            present::listen_pointer(
                window,
                hitbox,
                Rc::new(move |input| inner.borrow_mut().pointer(input)),
                view,
            );
        }
    }
}

/// 注册两个演示 GPU 视图、打开命令与 Ribbon 入口（产品装配时调用；
/// 视图类型由本模块提供，Workbench 模块层不接触 GUI，故不放在 wb-example）。
pub fn register_views(reg: &mut wb::Registry) {
    reg.register_view(
        wb::ViewDef::new("example.gpu3d", "三维视图 (wgpu)", true, || {
            Box::new(GpuViewportView::new3d())
        }),
    );
    reg.register_view(
        wb::ViewDef::new("example.gpu_top", "俯视图 (wgpu)", true, || {
            Box::new(GpuViewportView::new_top())
        }),
    );
    reg.register_command(wb::CommandDef::sync(
        "example.open_gpu3d",
        "三维视图",
        |ctx: &mut wb::CommandCtx, _a: &wb::CommandArgs| match ctx.open_view("example.gpu3d", false)
        {
            Ok(_) => wb::CommandResult::done(),
            Err(e) => wb::CommandResult::failed(e),
        },
    ));
    reg.register_command(wb::CommandDef::sync(
        "example.open_gpu_top",
        "俯视图",
        |ctx: &mut wb::CommandCtx, _a: &wb::CommandArgs| match ctx.open_view(
            "example.gpu_top", false,
        ) {
            Ok(_) => wb::CommandResult::done(),
            Err(e) => wb::CommandResult::failed(e),
        },
    ));
    reg.add_ribbon_tab(
        wb::RibbonTab::new("wb.gpu", "三维")
            .from_source("gpu-view")
            .groups(vec![wb::ToolbarGroup::new("gpu.views", "视图")
                .from_source("gpu-view")
                .items(vec![
                    wb::ToolbarItem::labeled("example.open_gpu3d", "三维视图"),
                    wb::ToolbarItem::labeled("example.open_gpu_top", "俯视图"),
                ])]),
    );
}
