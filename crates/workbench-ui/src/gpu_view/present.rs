//! How a GPU viewport's frames reach the screen.
//!
//! GPUI's `Element::paint` only records primitives; it never hands out a
//! render pass. So a viewport renders with wgpu itself and gives GPUI the
//! result in whatever form the platform can take:
//!
//! * **Web** ([`Presenter::Texture`]): GPUI's renderer *is* wgpu. The viewport
//!   renders during paint into a `WgpuDynamicTexture` on GPUI's own device and
//!   paints it as a sprite, so it joins GPUI's painter order like any element.
//! * **Windows** ([`Presenter::Native`]): GPUI renders with Direct3D 11. The
//!   viewport gets a native surface in GPUI's window composition — a
//!   DirectComposition visual between GPUI's base content and its overlays
//!   (menus, popovers, dialogs, anything `deferred`) — and presents a DX12
//!   swap chain of its own there. Frames never leave the GPU, and input-driven
//!   frames are presented without a GPUI frame at all. This follows
//!   javagg/gpui-fast-wgpu-demo.
//! * **Elsewhere, or if composition fails** ([`Presenter::Readback`]): render
//!   offscreen, read the pixels back and paint them as an image.
//!
//! What the viewport draws is the caller's: every path hands it a color view
//! in [`Gpu::target_format`] and its size in device pixels.

use std::rc::Rc;
#[cfg(not(target_family = "wasm"))]
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    App, Bounds, DevicePixels, DispatchPhase, EntityId, Hitbox, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, Window,
};

use super::Gpu;

/// A pointer event over a viewport, in window logical pixels. Only what the
/// viewport cameras use: left-button drags and the wheel.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PointerInput {
    /// The left button went down over the viewport.
    Press(Point<Pixels>),
    /// The pointer moved; `dragging` while a button is held.
    Move {
        position: Point<Pixels>,
        dragging: bool,
        shift: bool,
    },
    /// The left button came up.
    Release,
    /// The wheel turned over the viewport: pixels, positive toward the user.
    Scroll(f32),
}

/// What a viewport does with pointer input: true when its view changed and
/// needs a new frame.
pub(crate) type PointerSink = Rc<dyn Fn(PointerInput) -> bool>;

/// Wheel delta in pixels (a line counts as 20, as before the migration).
fn scroll_pixels(delta: ScrollDelta) -> f32 {
    match delta {
        ScrollDelta::Pixels(p) => f32::from(p.y),
        ScrollDelta::Lines(p) => p.y * 20.0,
    }
}

/// Device-pixel size of `bounds`, at least 1×1 and at most `max` per side.
pub(crate) fn device_size(bounds: Bounds<Pixels>, scale: f32, max: u32) -> (u32, u32) {
    let device: Bounds<DevicePixels> = bounds.to_device_pixels(scale);
    (
        (device.size.width.0.max(1) as u32).min(max),
        (device.size.height.0.max(1) as u32).min(max),
    )
}

/// Feeds a viewport's pointer input from ordinary GPUI listeners (the texture
/// and readback paths), asking the hosting view for a new frame when the
/// sink reports a change. Registered during paint, as GPUI clears listeners
/// every frame.
pub(crate) fn listen_pointer(window: &mut Window, hitbox: &Hitbox, sink: PointerSink, view: EntityId) {
    let redraw = move |changed: bool, cx: &mut App| {
        if changed {
            cx.notify(view);
        }
    };
    window.on_mouse_event({
        let (sink, hitbox) = (sink.clone(), hitbox.clone());
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble
                && event.button == MouseButton::Left
                && hitbox.is_hovered(window)
            {
                window.capture_pointer(hitbox.id);
                redraw(sink(PointerInput::Press(event.position)), cx);
            }
        }
    });
    window.on_mouse_event({
        let sink = sink.clone();
        move |event: &MouseMoveEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble {
                redraw(
                    sink(PointerInput::Move {
                        position: event.position,
                        dragging: event.dragging(),
                        shift: event.modifiers.shift,
                    }),
                    cx,
                );
            }
        }
    });
    window.on_mouse_event({
        let sink = sink.clone();
        move |event: &MouseUpEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                redraw(sink(PointerInput::Release), cx);
            }
        }
    });
    window.on_mouse_event({
        let hitbox = hitbox.clone();
        move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                redraw(sink(PointerInput::Scroll(scroll_pixels(event.delta))), cx);
            }
        }
    });
}

/// One viewport's way to the screen. See the module docs.
pub(crate) enum Presenter {
    /// Nothing tried yet: the first prepaint picks the path.
    Pending,
    #[cfg(target_family = "wasm")]
    Texture(texture::TexturePresenter),
    #[cfg(target_os = "windows")]
    Native(native::NativePresenter),
    #[cfg(not(target_family = "wasm"))]
    Readback(ReadbackPresenter),
}

impl Presenter {
    /// Whether pointer input arrives through the native surface rather than
    /// GPUI listeners. Over a native surface GPUI skips the bubble phase, so
    /// the element must not rely on its own listeners there.
    pub(crate) fn native_input(&self) -> bool {
        match self {
            #[cfg(target_os = "windows")]
            Self::Native(_) => true,
            _ => false,
        }
    }

    /// Prepaint: pick the path on first use and, for a native surface, place
    /// it where GPUI laid the element out (this must run in prepaint, which
    /// is what stacks the surface with its host).
    ///
    /// `native` is how a native surface renders and takes input on its own;
    /// it is used once, when that surface is created.
    #[allow(unused_variables)]
    pub(crate) fn prepaint(
        &mut self,
        gpu: &Rc<Gpu>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
        native: impl FnOnce() -> NativeHooks,
    ) {
        if matches!(self, Self::Pending) {
            *self = Self::choose(gpu, window, cx, native);
        }
        match self {
            #[cfg(target_os = "windows")]
            Self::Native(presenter) => {
                if let Err(error) = presenter.place(bounds, window.scale_factor()) {
                    eprintln!("gpu viewport: placing the native surface: {error:#}");
                }
            }
            _ => {}
        }
    }

    #[allow(unused_variables)]
    fn choose(
        gpu: &Rc<Gpu>,
        window: &mut Window,
        cx: &mut App,
        native: impl FnOnce() -> NativeHooks,
    ) -> Self {
        #[cfg(target_family = "wasm")]
        let presenter = Self::Texture(texture::TexturePresenter::default());
        #[cfg(target_os = "windows")]
        let presenter = match native::NativePresenter::new(gpu.clone(), window, cx, native()) {
            Ok(presenter) => Self::Native(presenter),
            Err(error) => {
                eprintln!(
                    "gpu viewport: window composition unavailable, reading frames back instead: {error:#}"
                );
                Self::Readback(ReadbackPresenter::default())
            }
        };
        #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
        let presenter = Self::Readback(ReadbackPresenter::default());
        presenter
    }

    /// Paint: render a frame if `dirty` (or the target had to change) and
    /// show the current one. `draw` renders into the given view.
    #[allow(unused_variables)]
    pub(crate) fn paint(
        &mut self,
        gpu: &Gpu,
        bounds: Bounds<Pixels>,
        dirty: bool,
        window: &mut Window,
        draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32)),
    ) -> Result<()> {
        match self {
            Self::Pending => Ok(()),
            #[cfg(target_family = "wasm")]
            Self::Texture(presenter) => presenter.paint(gpu, bounds, dirty, window, draw),
            #[cfg(target_os = "windows")]
            Self::Native(presenter) => presenter.paint(dirty, window, draw),
            #[cfg(not(target_family = "wasm"))]
            Self::Readback(presenter) => presenter.paint(gpu, bounds, dirty, window, draw),
        }
    }

    /// Native only: render a frame now, outside any GPUI frame. A no-op
    /// elsewhere, where frames are only drawn as GPUI paints.
    #[allow(unused_variables)]
    pub(crate) fn redraw_native(&mut self, draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32))) {
        match self {
            #[cfg(target_os = "windows")]
            Self::Native(presenter) => presenter.redraw(draw),
            _ => {}
        }
    }
}

/// How a native surface renders and takes input without a GPUI frame. Both
/// run on the UI thread, outside GPUI's draw, and must not hold the viewport
/// alive (capture a `Weak`), or a closed viewport would never give its
/// surface back.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) struct NativeHooks {
    /// Renders a frame now (through [`Presenter::redraw_native`]).
    pub redraw: Box<dyn FnMut()>,
    /// Takes pointer input over the surface.
    pub pointer: PointerSink,
}

// ---------------------------------------------------------------------------
// Readback: offscreen render, CPU readback, `paint_image`.

/// Renders offscreen and paints the pixels as an image. Every desktop can do
/// this; it costs a GPU→CPU copy and an atlas upload per frame.
#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
pub(crate) struct ReadbackPresenter {
    target: Option<ReadbackTarget>,
    image: Option<Arc<gpui::RenderImage>>,
}

#[cfg(not(target_family = "wasm"))]
impl ReadbackPresenter {
    fn paint(
        &mut self,
        gpu: &Gpu,
        bounds: Bounds<Pixels>,
        dirty: bool,
        window: &mut Window,
        draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32)),
    ) -> Result<()> {
        let max = gpu.device.limits().max_texture_dimension_2d.min(4096);
        let size = device_size(bounds, window.scale_factor(), max);
        let resized = self.target.as_ref().is_none_or(|t| t.size != size);
        if resized {
            self.target = Some(ReadbackTarget::new(&gpu.device, gpu.target_format, size));
        }
        if resized || dirty || self.image.is_none() {
            let target = self.target.as_ref().expect("allocated above");
            draw(&target.view, size);
            let bytes = target.read(gpu)?;
            let frame = image::Frame::new(
                image::ImageBuffer::from_raw(size.0, size.1, bytes)
                    .ok_or_else(|| anyhow::anyhow!("readback size mismatch"))?,
            );
            let image = Arc::new(gpui::RenderImage::new(vec![frame]));
            // Each frame is a new image; drop the last one from the sprite
            // atlas, or every frame would stay there.
            if let Some(previous) = self.image.replace(image) {
                window.drop_image(previous).ok();
            }
        }
        let image = self.image.clone().expect("rendered above");
        window.paint_image(bounds, bounds, gpui::Corners::all(gpui::px(0.)), image, 0, false)
    }
}

/// An offscreen color target with a readback buffer.
#[cfg(not(target_family = "wasm"))]
struct ReadbackTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    padded_bytes_per_row: u32,
    size: (u32, u32),
}

#[cfg(not(target_family = "wasm"))]
impl ReadbackTarget {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let (width, height) = size;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport-readback-color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_bytes_per_row = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport-readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            texture,
            view,
            buffer,
            padded_bytes_per_row,
            size,
        }
    }

    /// Copies the target out and waits for it: tightly packed BGRA rows.
    fn read(&self, gpu: &Gpu) -> Result<Vec<u8>> {
        let (width, height) = self.size;
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewport-readback"),
            });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);

        let (sender, receiver) = std::sync::mpsc::channel();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })?;
        receiver
            .recv()
            .map_err(|_| anyhow::anyhow!("readback channel closed"))??;

        let data = self.buffer.slice(..).get_mapped_range()?;
        let row = (width * 4) as usize;
        let mut packed = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let start = y * self.padded_bytes_per_row as usize;
            packed.extend_from_slice(&data[start..start + row]);
        }
        drop(data);
        self.buffer.unmap();
        Ok(packed)
    }
}

// ---------------------------------------------------------------------------
// Web: a dynamic texture on GPUI's device.

#[cfg(target_family = "wasm")]
mod texture {
    use anyhow::Result;
    use gpui::{Bounds, DevicePixels, Pixels, Window, size};
    use gpui_wgpu::WgpuDynamicTexture;

    use super::{Gpu, device_size};

    /// Renders during paint into a texture GPUI samples as a sprite.
    #[derive(Default)]
    pub(crate) struct TexturePresenter {
        texture: Option<(WgpuDynamicTexture, (u32, u32))>,
    }

    impl TexturePresenter {
        pub(super) fn paint(
            &mut self,
            gpu: &Gpu,
            bounds: Bounds<Pixels>,
            dirty: bool,
            window: &mut Window,
            draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32)),
        ) -> Result<()> {
            let max = gpu.device.limits().max_texture_dimension_2d.min(4096);
            let target = device_size(bounds, window.scale_factor(), max);
            let resized = self
                .texture
                .as_ref()
                .is_none_or(|(_, allocated)| *allocated != target);
            if resized {
                let texture = gpu.context.create_dynamic_texture(size(
                    DevicePixels(target.0 as i32),
                    DevicePixels(target.1 as i32),
                ))?;
                self.texture = Some((texture, target));
            }
            let (texture, _) = self.texture.as_ref().expect("allocated above");
            if resized || dirty {
                // Submits on GPUI's queue before GPUI's own submission samples
                // the texture, so no wait or copy is needed.
                texture.render(window, |view| {
                    draw(view, target);
                    Ok(())
                })?;
            }
            texture.paint(window, bounds)
        }
    }
}

// ---------------------------------------------------------------------------
// Windows: a DX12 swap chain on a DirectComposition visual.

#[cfg(target_os = "windows")]
mod native {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use anyhow::{Context as _, Result, anyhow};
    use gpui::{
        App, Bounds, DevicePixels, DispatchEventResult, ForegroundExecutor, MouseButton,
        MouseMoveEvent, Pixels, PlatformInput, Point, Window, WindowCompositionSurface, WindowId,
    };
    use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;
    use windows::core::{IUnknown, Interface as _};

    use super::{Gpu, NativeHooks, PointerInput, device_size, scroll_pixels};

    thread_local! {
        /// Native surfaces of closed viewports, by window. gpui-fast cannot
        /// remove a surface on Windows (`remove_surface` hides it first, and
        /// hiding casts the visual to `IDCompositionVisual3`, which visuals
        /// of its `DCompositionCreateDevice` device don't implement:
        /// E_NOINTERFACE), so they are parked off screen and reused.
        static SPARE: RefCell<Vec<(WindowId, WindowCompositionSurface)>> =
            const { RefCell::new(Vec::new()) };
    }

    /// The surface's GPUI side and its swap chain, shared with the input
    /// handler, the redraw task and the frame-liveness tokens.
    struct Shared {
        gpu: Rc<Gpu>,
        window: WindowId,
        surface: WindowCompositionSurface,
        swap_chain: wgpu::Surface<'static>,
        config: RefCell<wgpu::SurfaceConfiguration>,
        /// Configured since the last present: GPUI must commit its
        /// DirectComposition device after the next one, as wgpu sets the
        /// swap chain as the visual's content but cannot commit a device it
        /// doesn't own.
        needs_commit: Cell<bool>,
        /// Where the element was laid out, and at what scale.
        geometry: Cell<(Bounds<Pixels>, f32)>,
        /// Whether the surface has its bounds (rather than empty ones).
        shown: Cell<bool>,
        /// Liveness tokens held by frames that include the element.
        live: Cell<usize>,
        /// A redraw task is queued.
        redraw_queued: Cell<bool>,
        redraw: RefCell<Box<dyn FnMut()>>,
        executor: ForegroundExecutor,
    }

    impl Shared {
        /// Hides the surface: GPUI neither shows nor routes input to empty
        /// bounds. (`set_visible` fails on Windows; see [`SPARE`].)
        fn hide(&self) {
            if self.shown.replace(false) {
                let hidden = self
                    .surface
                    .platform_surface()
                    .and_then(|surface| surface.set_bounds(Bounds::default()));
                if let Err(error) = hidden {
                    eprintln!("gpu viewport: hiding the native surface: {error:#}");
                }
            }
        }

        /// Makes GPUI commit its composition device. Setting the parent origin
        /// is the public call that commits even when nothing changed (an
        /// implementation detail of gpui-fast's Windows backend, which the
        /// wgpu demo relies on too).
        fn commit(&self) {
            let committed = self
                .surface
                .platform_surface()
                .and_then(|surface| surface.set_parent_origin(Point::default()));
            if let Err(error) = committed {
                eprintln!("gpu viewport: committing the composition: {error:#}");
            }
        }

        fn queue_redraw(self: &Rc<Self>) {
            if self.redraw_queued.replace(true) {
                return;
            }
            let shared = Rc::downgrade(self);
            self.executor
                .spawn(async move {
                    if let Some(shared) = shared.upgrade() {
                        shared.redraw_queued.set(false);
                        (shared.redraw.borrow_mut())();
                    }
                })
                .detach();
        }

        /// Renders and presents one frame.
        fn render(&self, draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32))) {
            if !self.shown.get() {
                return;
            }
            let config = self.config.borrow();
            let frame = match self.swap_chain.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(frame)
                | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    self.swap_chain.configure(&self.gpu.device, &config);
                    self.needs_commit.set(true);
                    return;
                }
                // Occluded or timed out: the next change draws again.
                _ => return,
            };
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            draw(&view, (config.width, config.height));
            self.gpu.queue.present(frame);
            if self.needs_commit.replace(false) {
                self.commit();
            }
        }
    }

    /// Whether this thread's locals are being torn down. At process exit the
    /// application can be dropped by thread-local destructors, after [`SPARE`]
    /// itself; there is no window left to hide anything in then, and touching
    /// a destroyed thread local would abort the process.
    fn tearing_down() -> bool {
        SPARE.try_with(|_| ()).is_err()
    }

    impl Drop for Shared {
        fn drop(&mut self) {
            if tearing_down() {
                return;
            }
            self.hide();
            let spare = (self.window, self.surface.clone());
            let _ = SPARE.try_with(|pool| pool.borrow_mut().push(spare));
        }
    }

    /// Held by the mouse listener a painted viewport registers, which GPUI
    /// keeps for exactly as long as a frame includes the element: a frame
    /// that reuses a retained view moves its listeners along, and the frame
    /// that replaces the last one including the element drops them. When the
    /// last token goes, the element is gone from the screen (its tab was
    /// switched away, its panel closed or collapsed) and the surface, which
    /// GPUI would otherwise leave where it was, is hidden.
    struct LiveToken(Rc<Shared>);

    impl LiveToken {
        fn new(shared: &Rc<Shared>) -> Self {
            shared.live.set(shared.live.get() + 1);
            Self(shared.clone())
        }
    }

    impl Drop for LiveToken {
        fn drop(&mut self) {
            let live = self.0.live.get() - 1;
            self.0.live.set(live);
            if live == 0 && !tearing_down() {
                // Dropped while GPUI clears a frame: act once it is done.
                let shared = Rc::downgrade(&self.0);
                self.0
                    .executor
                    .spawn(async move {
                        if let Some(shared) = shared.upgrade() {
                            if shared.live.get() == 0 {
                                shared.hide();
                            }
                        }
                    })
                    .detach();
            }
        }
    }

    pub(crate) struct NativePresenter {
        shared: Rc<Shared>,
        /// Painted at least once since it was (re)configured.
        drawn: bool,
    }

    impl NativePresenter {
        pub(super) fn new(
            gpu: Rc<Gpu>,
            window: &mut Window,
            cx: &mut App,
            hooks: NativeHooks,
        ) -> Result<Self> {
            let window_id = window.window_handle().window_id();
            let composition = window.enable_window_composition()?;
            let spare = SPARE.with(|pool| {
                let mut pool = pool.borrow_mut();
                let index = pool.iter().position(|(id, _)| *id == window_id)?;
                Some(pool.swap_remove(index).1)
            });
            let surface = match spare {
                Some(surface) => surface,
                None => composition.create_native_surface()?,
            };

            let handle = surface.platform_surface()?.platform_handle()?;
            let visual: IDCompositionVisual = handle
                .downcast::<IUnknown>()
                .map_err(|_| anyhow!("the native surface is not a DirectComposition visual"))?
                .cast()
                .context("casting to IDCompositionVisual")?;
            // SAFETY: wgpu takes its own reference to the visual and holds it
            // for as long as the swap chain lives.
            let swap_chain = unsafe {
                gpu.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(
                    visual.as_raw(),
                ))
            }
            .context("creating a wgpu surface on the composition visual")?;
            let config = surface_config(&gpu, &swap_chain)?;
            swap_chain.configure(&gpu.device, &config);

            let shared = Rc::new(Shared {
                gpu,
                window: window_id,
                surface: surface.clone(),
                swap_chain,
                config: RefCell::new(config),
                needs_commit: Cell::new(true),
                geometry: Cell::new((Bounds::default(), window.scale_factor())),
                shown: Cell::new(false),
                live: Cell::new(0),
                redraw_queued: Cell::new(false),
                redraw: RefCell::new(hooks.redraw),
                executor: cx.foreground_executor().clone(),
            });

            // Pointer input over the surface comes here rather than to GPUI
            // listeners, in window logical pixels.
            let pointer = hooks.pointer;
            let weak = Rc::downgrade(&shared);
            composition.on_input(surface.id(), move |event| {
                let input = match event {
                    PlatformInput::MouseDown(event) if event.button == MouseButton::Left => {
                        PointerInput::Press(event.position)
                    }
                    PlatformInput::MouseUp(event) if event.button == MouseButton::Left => {
                        PointerInput::Release
                    }
                    PlatformInput::MouseMove(event) => PointerInput::Move {
                        position: event.position,
                        dragging: event.dragging(),
                        shift: event.modifiers.shift,
                    },
                    PlatformInput::Scroll(event) => PointerInput::Scroll(scroll_pixels(event.delta)),
                    _ => {
                        return DispatchEventResult {
                            propagate: true,
                            default_prevented: false,
                        };
                    }
                };
                if pointer(input) {
                    if let Some(shared) = weak.upgrade() {
                        shared.queue_redraw();
                    }
                }
                DispatchEventResult {
                    propagate: false,
                    default_prevented: false,
                }
            })?;

            Ok(Self {
                shared,
                drawn: false,
            })
        }

        /// Puts the surface where GPUI laid the element out, resizing the swap
        /// chain to match.
        pub(super) fn place(&mut self, bounds: Bounds<Pixels>, scale: f32) -> Result<()> {
            let shared = &self.shared;
            shared.geometry.set((bounds, scale));
            let device: Bounds<DevicePixels> = bounds.to_device_pixels(scale);
            if device.size.width.0 <= 0 || device.size.height.0 <= 0 {
                shared.hide();
                return Ok(());
            }
            shared.surface.platform_surface()?.set_bounds(device)?;
            shared.shown.set(true);

            let max = shared.gpu.device.limits().max_texture_dimension_2d;
            let (width, height) = device_size(bounds, scale, max);
            let mut config = shared.config.borrow_mut();
            if (config.width, config.height) != (width, height) {
                config.width = width;
                config.height = height;
                shared.swap_chain.configure(&shared.gpu.device, &config);
                shared.needs_commit.set(true);
                self.drawn = false;
            }
            Ok(())
        }

        pub(super) fn paint(
            &mut self,
            dirty: bool,
            window: &mut Window,
            draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32)),
        ) -> Result<()> {
            // The frame keeps this listener, and with it the token, for as
            // long as it includes the element.
            let token = LiveToken::new(&self.shared);
            window.on_mouse_event(move |_: &MouseMoveEvent, _, _, _| {
                let _ = &token;
            });
            if dirty || !self.drawn {
                self.shared.render(draw);
                self.drawn = true;
            }
            Ok(())
        }

        pub(super) fn redraw(&mut self, draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32))) {
            self.shared.render(draw);
        }
    }

    fn surface_config(gpu: &Gpu, swap_chain: &wgpu::Surface<'_>) -> Result<wgpu::SurfaceConfiguration> {
        let capabilities = swap_chain.get_capabilities(&gpu.adapter);
        anyhow::ensure!(
            capabilities.formats.contains(&gpu.target_format),
            "the composition swap chain cannot use {:?}",
            gpu.target_format
        );
        // Frames are presented on the UI thread, so don't wait for vblank:
        // Mailbox replaces a queued frame instead of blocking. DWM composes
        // the visual, so nothing tears either way.
        let present_mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Fifo]
            .into_iter()
            .find(|mode| capabilities.present_modes.contains(mode))
            .unwrap_or(wgpu::PresentMode::Fifo);
        Ok(wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: gpu.target_format,
            width: 1,
            height: 1,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: Vec::new(),
        })
    }
}
