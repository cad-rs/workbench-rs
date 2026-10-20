//! GPU 视口（前端原生视图扩展）：自建 wgpu 管线渲染 3D/2D 视图，
//! Windows / Web 复用 GPUI 渲染器的共享设备零拷贝合成（`paint_surface`），
//! 其余桌面自建独立设备离屏回读（`paint_image`）。
//! 移植自 gpui-wgpu 参考实现（javagg/gpui-demo）的 viewport/gpu/camera/mesh/scene。
//!
//! 集成方式：[`GpuViewportView`] 实现 [`wb::ViewInstance`]，其 `element`
//! 返回一个 [`GpuViewportElement`]——一个自绘的 gpui `Element`，在 `paint`
//! 阶段把一帧交给 `paint_surface` 合成。
//!
//! 设备不归属任何一个视图：[`shared_gpu`] 提供全进程共享的 [`Gpu`]，
//! 供插件自带的自定义 gpui `Element` 建自己的渲染资源。

mod camera;
mod gpu;
mod mesh;
mod scene;

use std::cell::RefCell;
use std::rc::Rc;

use camera::{OrthoCamera, OrbitCamera};
use gpui::{
    App, AnyElement, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollDelta,
    ScrollWheelEvent, Style, Window,
};
use scene::{Pipelines, Scene, Uniforms, ViewState};
use workbench_api as wb;

pub use gpu::Gpu;

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
/// `Failed` only applies to the standalone-device path, so a device creation
/// failure does not retry (and block) every frame.
enum GpuSlot {
    /// Windows / Web: renderer not ready yet, retry on the next frame.
    NotTried,
    Ready(Rc<Gpu>),
    #[cfg_attr(any(target_family = "wasm", target_os = "windows"), allow(dead_code))]
    Failed,
}

thread_local! {
    static SHARED_GPU: RefCell<GpuSlot> = const { RefCell::new(GpuSlot::NotTried) };
}

/// Acquire the process-wide GPU context, initializing it from `window` on
/// first use.
///
/// A plugin-provided WebGPU view calls this from its `Element::paint` and
/// builds its own pipelines, textures and render targets on the returned
/// device. Sharing is not optional on Windows / Web: `paint_surface` composites
/// a texture owned by gpui's renderer, so the texture must live on that
/// renderer's device.
///
/// Returns `None` until gpui's renderer is ready; callers should skip the
/// frame and retry later rather than treating it as an error.
pub fn shared_gpu(window: &Window) -> Option<Rc<Gpu>> {
    SHARED_GPU.with(|cell| {
        let mut slot = cell.borrow_mut();
        if matches!(&*slot, GpuSlot::NotTried) {
            #[cfg(any(target_family = "wasm", target_os = "windows"))]
            {
                // The renderer may not be ready on the first frame: stay in
                // NotTried so the next frame retries.
                if let Some(gpu) = Gpu::from_window(window) {
                    *slot = GpuSlot::Ready(Rc::new(gpu));
                }
            }
            #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
            {
                match Gpu::new() {
                    Ok(gpu) => *slot = GpuSlot::Ready(Rc::new(gpu)),
                    Err(err) => {
                        eprintln!("gpu viewport: failed to init wgpu device: {err}");
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

/// web 端自备的深度缓冲（WgpuRenderTarget 只有颜色）
#[cfg(any(target_family = "wasm", target_os = "windows"))]
struct DepthTarget {
    view: wgpu::TextureView,
}

#[cfg(any(target_family = "wasm", target_os = "windows"))]
impl DepthTarget {
    fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let (w, h) = size;
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport-depth"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
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
        }
    }
}

/// Mutable viewport state, shared between the view and its element.
///
/// The element needs live access to the camera and the GPU resources, so the
/// state lives behind an `Rc` that both sides clone.
struct GpuViewportInner {
    orbit: OrbitCamera,
    ortho: OrthoCamera,
    view_state: Option<ViewState>,
    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
    target: Option<scene::Target>,
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    web_target: Option<gpui_wgpu::WgpuRenderTarget>,
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    depth: Option<DepthTarget>,
    /// Drag state (pointer position of the previous frame).
    dragging: bool,
    last_pos: (f32, f32),
    dirty: bool,
    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
    image: Option<std::sync::Arc<gpui::RenderImage>>,
}

impl GpuViewportInner {
    fn new() -> Self {
        Self {
            orbit: OrbitCamera::new(),
            ortho: OrthoCamera::new(),
            view_state: None,
            #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
            target: None,
            #[cfg(any(target_family = "wasm", target_os = "windows"))]
            web_target: None,
            #[cfg(any(target_family = "wasm", target_os = "windows"))]
            depth: None,
            dragging: false,
            last_pos: (0.0, 0.0),
            dirty: true,
            #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
            image: None,
        }
    }

    fn ensure_view_state(&mut self, common: &ViewportCommon) {
        if self.view_state.is_none() {
            self.view_state = Some(ViewState::new(
                &common.gpu.device,
                &common.pipes.layout,
            ));
        }
    }

    fn current_uniforms(&self, kind: ViewKind, aspect: f32) -> Uniforms {
        match kind {
            ViewKind::Orbit3d => {
                let eye = self.orbit.eye();
                Uniforms::new(
                    self.orbit.view_proj(aspect),
                    eye,
                    glam::Vec3::new(0.45, 0.8, 0.35).normalize(),
                    0.002,
                )
            }
            ViewKind::Top2d => Uniforms::new(
                self.ortho.view_proj(aspect),
                glam::Vec3::new(self.ortho.center.x, 1000.0, self.ortho.center.z),
                // Light straight down the view direction: an elevation-drawing
                // style flat projection.
                glam::Vec3::new(0.0, -1.0, 0.0),
                0.002,
            ),
        }
    }

    /// 桌面端（非 Windows）：需要时渲染新帧并回读；返回当前应显示的图像。
    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
    fn render_if_needed(
        &mut self,
        kind: ViewKind,
        common: &ViewportCommon,
        width_px: u32,
        height_px: u32,
    ) -> anyhow::Result<std::sync::Arc<gpui::RenderImage>> {
        let size = (width_px.max(1), height_px.max(1));
        let resized = self.target.as_ref().is_none_or(|t| t.size != size);
        if resized {
            self.target = Some(scene::Target::new(
                &common.gpu.device,
                common.gpu.target_format,
                size,
            ));
            self.dirty = true;
        }

        if !self.dirty {
            if let Some(img) = self.image.clone() {
                return Ok(img);
            }
        }

        self.ensure_view_state(common);
        let view_state = self.view_state.as_ref().unwrap();
        let uniforms = self.current_uniforms(kind, size.0 as f32 / size.1 as f32);
        let target = self.target.as_mut().unwrap();

        let bytes = target.render_and_read(
            &common.gpu,
            &common.scene,
            &common.pipes,
            view_state,
            &uniforms,
        )?;
        let (w, h) = size;
        let frame = image::Frame::new(
            image::ImageBuffer::from_raw(w, h, bytes)
                .ok_or_else(|| anyhow::anyhow!("readback size mismatch"))?,
        );
        let img = std::sync::Arc::new(gpui::RenderImage::new(vec![frame]));
        self.image = Some(img.clone());
        self.dirty = false;
        Ok(img)
    }

    /// Windows / Web：把一帧渲染进 `WgpuRenderTarget`（零拷贝，由 paint_surface 合成）。
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    fn render_frame(&mut self, kind: ViewKind, common: &ViewportCommon, width_px: u32, height_px: u32) {
        let size = (width_px.max(1), height_px.max(1));
        let target_size =
            gpui::size(gpui::DevicePixels(size.0 as i32), gpui::DevicePixels(size.1 as i32));
        let resized = self
            .web_target
            .as_ref()
            .is_none_or(|t| t.size() != target_size);
        if resized {
            self.web_target = Some(gpui_wgpu::WgpuRenderTarget::new(
                common.gpu.context_handle(),
                target_size,
            ));
            self.depth = Some(DepthTarget::new(&common.gpu.device, size));
            self.dirty = true;
        }

        if !self.dirty {
            return;
        }

        self.ensure_view_state(common);
        let view_state = self.view_state.as_ref().unwrap();
        let web_target = self.web_target.as_ref().unwrap();
        let depth = self.depth.as_ref().unwrap();
        let uniforms = self.current_uniforms(kind, size.0 as f32 / size.1 as f32);

        common
            .gpu
            .queue
            .write_buffer(&view_state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = common
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewport-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: web_target.view(),
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
            scene::draw_scene(
                &mut pass,
                &common.scene,
                &common.pipes,
                &view_state.bind_group,
            );
        }
        common.gpu.queue.submit([encoder.finish()]);
        self.dirty = false;
    }

    /// Windows / Web：当前渲染目标的纹理表面（交给 paint_surface 合成）。
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    fn surface_source(&self) -> gpui::SurfaceSource {
        let web_target = self.web_target.as_ref().unwrap();
        gpui::SurfaceSource::Texture {
            texture: web_target.texture(),
            size: web_target.size(),
        }
    }

    /// Draw one frame into the view area. Draws a placeholder background until
    /// the process-wide GPU context is available.
    fn paint_into(&mut self, kind: ViewKind, window: &mut Window, bounds: Bounds<Pixels>) {
        let scale = window.scale_factor();
        let (w, h) = device_size(&bounds, scale);

        let Some(common) = demo_viewport(window) else {
            crate::paint_placeholder(window, bounds);
            return;
        };

        #[cfg(any(target_family = "wasm", target_os = "windows"))]
        {
            self.render_frame(kind, &common, w, h);
            if self.web_target.is_some() {
                let source = self.surface_source();
                window.paint_surface(bounds, source);
            }
        }
        #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
        {
            match self.render_if_needed(kind, &common, w, h) {
                Ok(img) => {
                    let _ = window.paint_image(
                        bounds,
                        bounds,
                        gpui::Corners::all(gpui::px(0.)),
                        img,
                        0,
                        false,
                    );
                }
                Err(err) => {
                    eprintln!("gpu viewport render error: {err}");
                    crate::paint_placeholder(window, bounds);
                }
            }
        }
    }
}

fn device_size(bounds: &Bounds<Pixels>, scale: f32) -> (u32, u32) {
    let w = (f32::from(bounds.size.width) * scale).round().max(1.0) as u32;
    let h = (f32::from(bounds.size.height) * scale).round().max(1.0) as u32;
    (w.min(4096), h.min(4096))
}

/// A GPU viewport view instance (central tab content).
struct GpuViewportView {
    kind: ViewKind,
    type_id: &'static str,
    title: &'static str,
    inner: Rc<RefCell<GpuViewportInner>>,
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
            kind,
            type_id,
            title,
            inner: Rc::new(RefCell::new(GpuViewportInner::new())),
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
            kind: self.kind,
            inner: self.inner.clone(),
        }
        .into_any_element();
        Some(Box::new(element))
    }
}

/// A gpui element that renders one GPU viewport.
///
/// Pointer input is registered during `paint` — gpui clears window listeners
/// after every frame — and drives the camera for the next frame.
struct GpuViewportElement {
    kind: ViewKind,
    inner: Rc<RefCell<GpuViewportInner>>,
}

impl IntoElement for GpuViewportElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for GpuViewportElement {
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
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // No children: the viewport fills whatever its parent gives it.
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        self.register_input(window, bounds);
        self.inner.borrow_mut().paint_into(self.kind, window, bounds);
    }
}

impl GpuViewportElement {
    /// Register the pointer handlers that drive this viewport's camera.
    fn register_input(&self, window: &mut Window, bounds: Bounds<Pixels>) {
        let origin = bounds.origin;

        let inner = self.inner.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, _window, _cx| {
            if phase.bubble() && ev.button == MouseButton::Left {
                let mut v = inner.borrow_mut();
                v.dragging = true;
                v.last_pos = (
                    f32::from(ev.position.x - origin.x),
                    f32::from(ev.position.y - origin.y),
                );
            }
        });

        let inner = self.inner.clone();
        let kind = self.kind;
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _window, _cx| {
            if !phase.bubble() || !ev.dragging() {
                return;
            }
            let mut v = inner.borrow_mut();
            let p = (
                f32::from(ev.position.x - origin.x),
                f32::from(ev.position.y - origin.y),
            );
            let (dx, dy) = (p.0 - v.last_pos.0, p.1 - v.last_pos.1);
            if dx != 0.0 || dy != 0.0 {
                match (kind, ev.modifiers.shift) {
                    (ViewKind::Orbit3d, false) => v.orbit.orbit(dx, dy),
                    (ViewKind::Orbit3d, true) => v.orbit.pan(dx, dy),
                    (ViewKind::Top2d, _) => v.ortho.pan(dx, dy),
                }
                v.dirty = true;
            }
            v.last_pos = p;
        });

        let inner = self.inner.clone();
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _window, _cx| {
            if phase.bubble() {
                inner.borrow_mut().dragging = false;
            }
        });

        let inner = self.inner.clone();
        let kind = self.kind;
        window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, _window, _cx| {
            if !phase.bubble() {
                return;
            }
            let delta = match ev.delta {
                ScrollDelta::Pixels(p) => f32::from(p.y),
                ScrollDelta::Lines(p) => p.y * 20.0,
            };
            let factor = (-delta * 0.0012).exp();
            let mut v = inner.borrow_mut();
            match kind {
                ViewKind::Orbit3d => v.orbit.zoom(factor),
                ViewKind::Top2d => v.ortho.zoom(factor),
            }
            v.dirty = true;
        });
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
