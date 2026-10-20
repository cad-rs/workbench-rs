//! GPU 视口（前端原生视图扩展）：自建 wgpu 管线渲染 3D/2D 视图，
//! Windows / Web 复用 GPUI 渲染器的共享设备零拷贝合成（`paint_surface`），
//! 其余桌面自建独立设备离屏回读（`paint_image`）。
//! 移植自 gpui-wgpu 参考实现（javagg/gpui-demo）的 viewport/gpu/camera/mesh/scene。
//!
//! 集成方式：[`GpuViewportView`] 实现 [`wb::ViewInstance`]，经
//! `ViewInstance::native_view` 类型擦除出口被前端识别（[`paint_native_view`]），
//! 命中后前端直接以 GPU 通道呈现，不再走 2D `PaintBackend`。

mod camera;
mod gpu;
mod mesh;
mod scene;

use std::cell::RefCell;
use std::rc::Rc;

use camera::{OrthoCamera, OrbitCamera};
use gpu::Gpu;
use gpui::{Bounds, Pixels, Window};
use scene::{Pipelines, Scene, Uniforms, ViewState};
use workbench_api as wb;

#[derive(Clone, Copy)]
enum ViewKind {
    /// 3D 轨道视图：左键拖拽旋转，Shift+左键平移，滚轮缩放
    Orbit3d,
    /// 俯视正交视图：左键拖拽平移，滚轮缩放
    Top2d,
}

/// 所有 GPU 视口共享的 GPU 资源（device/queue/scene/pipelines）。
/// Windows / Web 在首帧渲染器就绪后从窗口惰性获取；其余桌面自建设备。
struct ViewportCommon {
    gpu: Rc<Gpu>,
    scene: Rc<Scene>,
    pipes: Rc<Pipelines>,
}

impl ViewportCommon {
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    fn from_window(window: &Window) -> Option<Self> {
        let gpu = Rc::new(Gpu::from_window(window)?);
        let scene = Rc::new(Scene::new(&gpu.device));
        let pipes = Rc::new(Pipelines::new(&gpu.device, gpu.target_format));
        Some(Self { gpu, scene, pipes })
    }

    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
    fn standalone() -> anyhow::Result<Self> {
        let gpu = Rc::new(Gpu::new()?);
        let scene = Rc::new(Scene::new(&gpu.device));
        let pipes = Rc::new(Pipelines::new(&gpu.device, gpu.target_format));
        Ok(Self { gpu, scene, pipes })
    }
}

/// 共享 GPU 资源槽（渲染发生在 gpui 主线程）。
/// `Failed` 仅用于非 Windows 桌面的自建设备失败（避免每帧重试阻塞）。
enum SharedSlot {
    /// Windows / Web：渲染器未就绪，下一帧重试
    NotTried,
    Ready(Rc<ViewportCommon>),
    #[cfg_attr(any(target_family = "wasm", target_os = "windows"), allow(dead_code))]
    Failed,
}

thread_local! {
    static SHARED: RefCell<SharedSlot> = const { RefCell::new(SharedSlot::NotTried) };
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

/// 一个 GPU 视口视图实例（中央标签内容）。
struct GpuViewportView {
    kind: ViewKind,
    type_id: &'static str,
    title: &'static str,
    orbit: OrbitCamera,
    ortho: OrthoCamera,
    view_state: Option<ViewState>,
    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
    target: Option<scene::Target>,
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    web_target: Option<gpui_wgpu::WgpuRenderTarget>,
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    depth: Option<DepthTarget>,
    /// 拖拽状态（上一帧指针位置）
    dragging: bool,
    last_pos: (f32, f32),
    dirty: bool,
    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
    image: Option<std::sync::Arc<gpui::RenderImage>>,
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

    /// 消费本帧输入：拖拽（旋转/平移）与滚轮缩放。
    fn feed_input(&mut self, input: &wb::ViewInput) {
        if input.mouse_down {
            if let Some(p) = input.mouse_pos {
                if self.dragging {
                    let dx = p.x - self.last_pos.0;
                    let dy = p.y - self.last_pos.1;
                    if dx != 0.0 || dy != 0.0 {
                        match (self.kind, input.shift) {
                            (ViewKind::Orbit3d, false) => self.orbit.orbit(dx, dy),
                            (ViewKind::Orbit3d, true) => self.orbit.pan(dx, dy),
                            (ViewKind::Top2d, _) => self.ortho.pan(dx, dy),
                        }
                        self.dirty = true;
                    }
                    self.last_pos = (p.x, p.y);
                } else {
                    self.dragging = true;
                    self.last_pos = (p.x, p.y);
                }
            }
        } else {
            self.dragging = false;
        }
        if let Some(dy) = input.scroll {
            let factor = (-dy * 0.0012).exp();
            match self.kind {
                ViewKind::Orbit3d => self.orbit.zoom(factor),
                ViewKind::Top2d => self.ortho.zoom(factor),
            }
            self.dirty = true;
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

    fn current_uniforms(&self, aspect: f32) -> Uniforms {
        match self.kind {
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
                // 光从正上方（观察方向）打下来，得到工程图式的平面投影着色
                glam::Vec3::new(0.0, -1.0, 0.0),
                0.002,
            ),
        }
    }

    /// 桌面端（非 Windows）：需要时渲染新帧并回读；返回当前应显示的图像。
    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
    fn render_if_needed(
        &mut self,
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
        let uniforms = self.current_uniforms(size.0 as f32 / size.1 as f32);
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
    fn render_frame(&mut self, common: &ViewportCommon, width_px: u32, height_px: u32) {
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
        let uniforms = self.current_uniforms(size.0 as f32 / size.1 as f32);

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

    /// 在视图区域绘制一帧（惰性获取共享 GPU 资源；未就绪时画占位背景）。
    fn paint_into(&mut self, window: &mut Window, bounds: Bounds<Pixels>) {
        let scale = window.scale_factor();
        let (w, h) = device_size(&bounds, scale);

        SHARED.with(|cell| {
            let mut slot = cell.borrow_mut();
            // 惰性初始化共享 GPU 资源
            if matches!(&*slot, SharedSlot::NotTried) {
                #[cfg(any(target_family = "wasm", target_os = "windows"))]
                {
                    // 首帧渲染器可能未就绪：保持 NotTried，下一帧重试
                    if let Some(common) = ViewportCommon::from_window(window) {
                        *slot = SharedSlot::Ready(Rc::new(common));
                    }
                }
                #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
                {
                    match ViewportCommon::standalone() {
                        Ok(common) => *slot = SharedSlot::Ready(Rc::new(common)),
                        Err(err) => {
                            eprintln!("gpu viewport: failed to init wgpu device: {err}");
                            *slot = SharedSlot::Failed;
                        }
                    }
                }
            }
            match &mut *slot {
                SharedSlot::Ready(common) => {
                    #[cfg(any(target_family = "wasm", target_os = "windows"))]
                    {
                        self.render_frame(common, w, h);
                        if self.web_target.is_some() {
                            let source = self.surface_source();
                            window.paint_surface(bounds, source);
                        }
                    }
                    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
                    {
                        match self.render_if_needed(common, w, h) {
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
                _ => crate::paint_placeholder(window, bounds),
            }
        });
    }
}

fn device_size(bounds: &Bounds<Pixels>, scale: f32) -> (u32, u32) {
    let w = (f32::from(bounds.size.width) * scale).round().max(1.0) as u32;
    let h = (f32::from(bounds.size.height) * scale).round().max(1.0) as u32;
    (w.min(4096), h.min(4096))
}

impl wb::ViewInstance for GpuViewportView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new(self.type_id)
    }

    fn title(&self) -> String {
        self.title.to_string()
    }

    /// 2D 回退通道：仅在前端不识别原生视图时出现（正常 gpui 前端不会走到）。
    fn paint(&mut self, painter: &mut dyn wb::PaintBackend, rect: wb::Rect, _ctx: &mut wb::ViewCtx) {
        painter.fill_rect(rect, crate::theme::ui_theme().placeholder, 0.0);
        painter.text(
            wb::Point::new(rect.pos.x + 24.0, rect.pos.y + 24.0),
            14.0,
            wb::Color::GRAY,
            "当前前端未启用原生 GPU 视图通道（此视图需要 gpui + wgpu 前端）",
        );
    }

    fn native_view(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// 原生视图绘制入口：先识别前端自有的 [`GpuViewportView`]，再依次尝试
/// 外部 crate 登记过的呈现器（[`register_native_presenter`]）。
/// 返回 true 表示已消费（前端不应再走 2D `paint` 通道）。
pub(crate) fn paint_native_view(
    inst: &mut dyn wb::ViewInstance,
    window: &mut Window,
    bounds: Bounds<Pixels>,
    input: &wb::ViewInput,
) -> bool {
    let Some(native) = inst.native_view() else {
        return false;
    };
    if let Some(vp) = native.downcast_mut::<GpuViewportView>() {
        vp.feed_input(input);
        vp.paint_into(window, bounds);
        return true;
    }
    // 复制登记表再派发：绘制期间不持锁（呈现器内部若再注册不会死锁）。
    let registered = presenters()
        .lock()
        .expect("native presenter registry poisoned")
        .clone();
    for presenter in registered {
        if presenter(native, window, bounds, input) {
            return true;
        }
    }
    false
}

/// 前端原生视图呈现器：**外部 crate** 接入原生视图通道的通用出口。
///
/// `workbench-api` 只有 GUI 无关的
/// [`wb::ViewInstance::native_view`]（`&mut dyn Any`），具体类型由前端与其
/// 视图扩展约定；产品侧的视图类型实现本 trait 并在 workbench `init` 里登记
/// （[`register_native_presenter`]），前端即可命中原生通道，不再走 2D
/// `PaintBackend`。视图类型的 `native_view()` 返回 `Some(self)`。
pub trait NativeViewportPresenter: 'static {
    /// 以原生通道绘制一帧；`bounds` 是该视图在本帧的屏幕区域（逻辑像素）。
    fn paint_native(&mut self, window: &mut Window, bounds: Bounds<Pixels>, input: &wb::ViewInput);
}

/// 一次呈现尝试：命中本类型则绘制并返回 true。
type PresenterDispatch =
    fn(&mut dyn std::any::Any, &mut Window, Bounds<Pixels>, &wb::ViewInput) -> bool;

fn presenters() -> &'static std::sync::Mutex<Vec<PresenterDispatch>> {
    static PRESENTERS: std::sync::OnceLock<std::sync::Mutex<Vec<PresenterDispatch>>> =
        std::sync::OnceLock::new();
    PRESENTERS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// 登记一个外部原生视图呈现器（在 workbench `init` 的视图注册旁调用一次）。
pub fn register_native_presenter<T: NativeViewportPresenter>() {
    presenters()
        .lock()
        .expect("native presenter registry poisoned")
        .push(|any, window, bounds, input| match any.downcast_mut::<T>() {
            Some(view) => {
                view.paint_native(window, bounds, input);
                true
            }
            None => false,
        });
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
