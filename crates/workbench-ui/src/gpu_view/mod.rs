//! GPU viewport (a native view extension of the frontend): renders 3D/2D views with its own wgpu
//! pipelines.
//!
//! How a frame reaches the screen is decided per platform by the shared
//! [`gpui_gpu_view`] slot: the web renders on GPUI's own device into a
//! `WgpuDynamicTexture` (joining the draw order as a sprite); Windows takes a
//! native surface (a DirectComposition visual) in the GPUI window's composition and hangs the
//! viewport's own DX12 swap chain on it (above GPUI's base content, below menus / popovers /
//! dialogs); other desktops, or Windows without composition, render offscreen + read back into a
//! texture.
//!
//! Integration: [`GpuViewportView`] implements [`wb::ViewInstance`], and its `element` returns a
//! [`GpuViewportElement`], a self-drawn gpui `Element` that positions itself in prepaint and
//! renders in paint; presentation, input and dirty tracking are all handled by
//! [`gpui_gpu_view::Viewport`], and this module and [`host`] only supply their own
//! [`gpui_gpu_view::ViewportContent`].
//!
//! The device belongs to no single view: [`shared_gpu`] provides the process-wide shared [`Gpu`],
//! on which plugins' own custom gpui `Element`s build their render resources.

mod camera;
// Product injection contract (design doc, "GPU viewport injection API"): mesh/scene are public,
// so a product can build its own shaded mesh (Scene::from_mesh) and inject it into an orbit
// viewport via GpuViewportView::new3d_with_scene; the camera is still driven by the framework
// (reusing the Orbit3d interaction).
pub mod mesh;
pub mod scene;
// Generic viewport host Element (the shell around the workbench-render render core: raw input
// forwarding + per-platform presentation; the product passes a RenderScene + base view).
pub mod host;
pub use host::RenderViewportElement;

use std::cell::RefCell;
use std::rc::Rc;

use camera::{OrbitCamera, OrthoCamera};
use gpui::{
    App, AnyElement, Bounds, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, Pixels, Window,
};
use scene::{Pipelines, Scene, Uniforms, ViewState};
use workbench_api as wb;

pub use gpui_gpu_view::{
    set_gpu_requirements, set_wheel_line_pixels, shared_gpu, Gpu, GpuRequirements,
};
#[cfg(target_family = "wasm")]
pub use gpui_gpu_view::{set_web_platform, web_context};
use gpui_gpu_view::{Viewport, ViewportContent, ViewportInput};

#[derive(Clone, Copy)]
enum ViewKind {
    /// 3D orbit view: left-drag rotates, Shift+left-drag pans, the wheel zooms
    Orbit3d,
    /// Top-down orthographic view: left-drag pans, the wheel zooms
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

/// Product scene provider: called on every paint, returns the injected scene (`None` →
/// placeholder). The implementor does its own caching (deciding on rebuilds by content identity)
/// and returns an `Rc` so the pointer-identity comparison against the injection cache holds
/// (avoiding a rebuild of the upload buffers every frame).
pub type SceneProvider = dyn Fn(&wgpu::Device) -> Option<Rc<Scene>>;

/// What a demo (or product-injected) viewport draws, and its camera.
struct SceneView {
    kind: ViewKind,
    orbit: OrbitCamera,
    ortho: OrthoCamera,
    /// The scene drawn; `None` until the device (or the injected scene) is
    /// available, which paints the placeholder.
    common: Option<Rc<ViewportCommon>>,
    /// The product provider `common` was built from: a different one (by
    /// identity) rebuilds it.
    provider: Option<Rc<SceneProvider>>,
    view_state: Option<ViewState>,
    depth: Option<DepthTarget>,
    /// Where the pointer was last seen with the button held, for drag deltas.
    last_pointer: Option<gpui::Point<Pixels>>,
}

impl SceneView {
    fn new(kind: ViewKind) -> Self {
        Self {
            kind,
            orbit: OrbitCamera::new(),
            ortho: OrthoCamera::new(),
            common: None,
            provider: None,
            view_state: None,
            depth: None,
            last_pointer: None,
        }
    }

    /// Builds the scene on first use (or when the product's provider
    /// changed). True when it was (re)built and needs a new frame.
    fn resolve(&mut self, provider: Option<&Rc<SceneProvider>>, window: &Window) -> bool {
        let same = match (provider, &self.provider) {
            (Some(wanted), Some(built)) => Rc::ptr_eq(wanted, built),
            (None, None) => true,
            _ => false,
        };
        if same && self.common.is_some() {
            return false;
        }
        self.common = match provider {
            Some(provider) => shared_gpu(window).and_then(|gpu| {
                let scene = provider(&gpu.device)?;
                let pipes = Rc::new(Pipelines::new(&gpu.device, gpu.target_format));
                Some(Rc::new(ViewportCommon { gpu, scene, pipes }))
            }),
            None => demo_viewport(window),
        };
        self.provider = provider.cloned();
        self.view_state = self
            .common
            .as_ref()
            .map(|c| ViewState::new(&c.gpu.device, &c.pipes.layout));
        self.common.is_some()
    }
}

impl ViewportContent for SceneView {
    fn input(&mut self, input: ViewportInput) -> bool {
        if let Some(pixels) = input.scroll {
            let factor = (-pixels * 0.0012).exp();
            match self.kind {
                ViewKind::Orbit3d => self.orbit.zoom(factor),
                ViewKind::Top2d => self.ortho.zoom(factor),
            }
            return true;
        }
        let Some(position) = input.mouse_pos else {
            return false;
        };
        if !input.mouse_down {
            self.last_pointer = None;
            return false;
        }
        let changed = match self.last_pointer {
            Some(from) => {
                let dx = f32::from(position.x - from.x);
                let dy = f32::from(position.y - from.y);
                if dx == 0.0 && dy == 0.0 {
                    false
                } else {
                    match (self.kind, input.shift) {
                        (ViewKind::Orbit3d, false) => self.orbit.orbit(dx, dy),
                        (ViewKind::Orbit3d, true) => self.orbit.pan(dx, dy),
                        (ViewKind::Top2d, _) => self.ortho.pan(dx, dy),
                    }
                    true
                }
            }
            None => false,
        };
        self.last_pointer = Some(position);
        changed
    }

    fn draw(&mut self, gpu: &Gpu, view: &wgpu::TextureView, size: (u32, u32)) {
        let (Some(common), Some(view_state)) = (&self.common, &self.view_state) else {
            return;
        };
        let aspect = size.0 as f32 / size.1 as f32;
        let uniforms = match self.kind {
            ViewKind::Orbit3d => Uniforms::new(
                self.orbit.view_proj(aspect),
                self.orbit.eye(),
                glam::Vec3::new(0.45, 0.8, 0.35).normalize(),
                0.002,
            ),
            ViewKind::Top2d => Uniforms::new(
                self.ortho.view_proj(aspect),
                glam::Vec3::new(self.ortho.center.x, 1000.0, self.ortho.center.z),
                // Light straight down the view direction: an elevation-drawing
                // style flat projection.
                glam::Vec3::new(0.0, -1.0, 0.0),
                0.002,
            ),
        };
        if self.depth.as_ref().is_none_or(|d| d.size != size) {
            self.depth = Some(DepthTarget::new(&gpu.device, size));
        }
        let depth = self.depth.as_ref().expect("allocated above");
        gpu.queue
            .write_buffer(&view_state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
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
}

/// A GPU viewport view instance (central tab content).
pub struct GpuViewportView {
    type_id: &'static str,
    title: &'static str,
    viewport: Rc<RefCell<Viewport<SceneView>>>,
    scene_provider: Option<Rc<SceneProvider>>,
}

impl GpuViewportView {
    fn new(kind: ViewKind, type_id: &'static str, title: &'static str) -> Self {
        Self {
            type_id,
            title,
            viewport: Rc::new(RefCell::new(Viewport::new(SceneView::new(kind)))),
            scene_provider: None,
        }
    }

    /// A 3D orbit viewport for a product-injected scene (injection API): the product picks
    /// `type_id`/`title`; `scene_provider` is called on every paint (the implementor caches), and
    /// `None` shows the placeholder. Camera interaction (orbit/pan/zoom) reuses the framework's
    /// implementation.
    pub fn new3d_with_scene(
        type_id: &'static str,
        title: &'static str,
        scene_provider: Rc<SceneProvider>,
    ) -> Self {
        Self {
            scene_provider: Some(scene_provider),
            ..Self::new(ViewKind::Orbit3d, type_id, title)
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
            viewport: self.viewport.clone(),
            scene_provider: self.scene_provider.clone(),
        }
        .into_any_element();
        Some(Box::new(element))
    }
}

/// A gpui element that renders one GPU viewport: placed in prepaint, drawn
/// in paint (see the shared slot).
struct GpuViewportElement {
    viewport: Rc<RefCell<Viewport<SceneView>>>,
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
        // Element state keeps a native surface's liveness token.
        Some("gpu-viewport".into())
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
        (gpui_gpu_view::full_size_layout(window, cx), ())
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
        let gpu = {
            let mut viewport = self.viewport.borrow_mut();
            if viewport.content.resolve(self.scene_provider.as_ref(), window) {
                viewport.invalidate();
            }
            viewport.content.common.as_ref().map(|c| c.gpu.clone())
        };
        match gpu {
            Some(gpu) => Viewport::prepaint(&self.viewport, &gpu, bounds, window, cx),
            None => window.insert_hitbox(bounds, HitboxBehavior::Normal),
        }
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let ready = self.viewport.borrow().content.common.is_some();
        let id = id.expect("the element has an id");
        if !ready || !Viewport::paint(&self.viewport, id, bounds, hitbox, window) {
            crate::paint_placeholder(window, bounds);
        }
    }
}

/// Registers the two demo GPU views, their open commands and the Ribbon entries (called during
/// product assembly; the view types come from this module and the Workbench module layer doesn't
/// touch the GUI, so this doesn't live in wb-example).
pub fn register_views(reg: &mut wb::Registry) {
    reg.register_view(wb::ViewDef::new("example.gpu3d", "3D View (wgpu)", true, || {
        Box::new(GpuViewportView::new(ViewKind::Orbit3d, "example.gpu3d", "3D View (wgpu)"))
    }));
    reg.register_view(wb::ViewDef::new("example.gpu_top", "Top View (wgpu)", true, || {
        Box::new(GpuViewportView::new(ViewKind::Top2d, "example.gpu_top", "Top View (wgpu)"))
    }));
    for (command, title, view) in [
        ("example.open_gpu3d", "3D View", "example.gpu3d"),
        ("example.open_gpu_top", "Top View", "example.gpu_top"),
    ] {
        reg.register_command(wb::CommandDef::sync(
            command,
            title,
            move |ctx: &mut wb::CommandCtx, _a: &wb::CommandArgs| match ctx.open_view(view, false) {
                Ok(_) => wb::CommandResult::done(),
                Err(e) => wb::CommandResult::failed(e),
            },
        ));
    }
    reg.add_ribbon_tab(
        wb::RibbonTab::new("wb.gpu", "3D")
            .from_source("gpu-view")
            .groups(vec![wb::ToolbarGroup::new("gpu.views", "Views")
                .from_source("gpu-view")
                .items(vec![
                    wb::ToolbarItem::labeled("example.open_gpu3d", "3D View"),
                    wb::ToolbarItem::labeled("example.open_gpu_top", "Top View"),
                ])]),
    );
    reg.register_locales(
        "zh-CN",
        [
            ("3D View (wgpu)", "三维视图 (wgpu)"),
            ("Top View (wgpu)", "俯视图 (wgpu)"),
            ("3D View", "三维视图"),
            ("Top View", "俯视图"),
            ("3D", "三维"),
            ("Views", "视图"),
        ],
    );
}
