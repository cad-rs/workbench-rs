//! How a GPU viewport's frames reach the screen, and the shell every viewport
//! element shares.
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
//! [`Viewport`] wraps a presenter with what every viewport needs — the dirty
//! flag, drag tracking and input routing — so an element only supplies its
//! [`ViewportContent`]: how a drag or the wheel moves its camera, and what it
//! draws into a color view in [`Gpu::target_format`].

use std::cell::RefCell;
use std::rc::{Rc, Weak};
#[cfg(not(target_family = "wasm"))]
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    App, Bounds, DevicePixels, DispatchPhase, GlobalElementId, Hitbox, HitboxBehavior, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollEvent, Style, Window,
};

use super::Gpu;

/// Offscreen targets (texture and readback paths) are capped at this many
/// device pixels per side.
const MAX_OFFSCREEN: u32 = 4096;

/// What a GPU viewport shows. [`Viewport`] does the rest.
pub(crate) trait ViewportContent: 'static {
    /// A left-button drag over the viewport moved by `dx`, `dy` logical
    /// pixels. True when the view changed.
    fn drag(&mut self, dx: f32, dy: f32, shift: bool) -> bool;
    /// The wheel turned over the viewport by `pixels`, positive toward the
    /// user. True when the view changed.
    fn scroll(&mut self, pixels: f32) -> bool;
    /// Renders a frame into `view` ([`Gpu::target_format`], `size` in device
    /// pixels), submitting before it returns.
    fn draw(&mut self, view: &wgpu::TextureView, size: (u32, u32));
}

/// A pointer event over a viewport, in window logical pixels.
#[derive(Clone, Copy, Debug)]
enum PointerInput {
    Press(Point<Pixels>),
    /// `dragging` while a button is held.
    Move {
        position: Point<Pixels>,
        dragging: bool,
        shift: bool,
    },
    Release,
    /// Pixels, positive toward the user (a line counts as 20).
    Scroll(f32),
}

fn scroll_pixels(delta: ScrollDelta) -> f32 {
    match delta {
        ScrollDelta::Pixels(p) => f32::from(p.y),
        ScrollDelta::Lines(p) => p.y * 20.0,
    }
}

/// What a native surface calls back into outside GPUI's frames.
trait ViewportInput {
    /// True when the view changed and needs a new frame.
    fn pointer(&mut self, input: PointerInput) -> bool;
    /// Renders a frame now.
    #[cfg(target_os = "windows")]
    fn redraw_native(&mut self);
}

/// A viewport: its content, how its frames reach the screen, and its input.
pub(crate) struct Viewport<C> {
    pub(crate) content: C,
    /// Chosen at the first prepaint that has a device.
    presenter: Option<Presenter>,
    dirty: bool,
    /// Where a left-button drag over the viewport last was.
    drag_from: Option<Point<Pixels>>,
}

impl<C: ViewportContent> Viewport<C> {
    pub(crate) fn new(content: C) -> Self {
        Self {
            content,
            presenter: None,
            dirty: true,
            drag_from: None,
        }
    }

    /// The content changed: the next paint renders a new frame.
    pub(crate) fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Prepaint: pick the way to the screen on first use and place a native
    /// surface where GPUI laid the element out (this must run in prepaint,
    /// which is what stacks the surface with its host). Returns the element's
    /// hitbox.
    pub(crate) fn prepaint(
        this: &Rc<RefCell<Self>>,
        gpu: &Rc<Gpu>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        let weak = Rc::downgrade(this);
        let owner: Weak<RefCell<dyn ViewportInput>> = weak;
        let mut guard = this.borrow_mut();
        let viewport = &mut *guard;
        let presenter = viewport
            .presenter
            .get_or_insert_with(|| Presenter::new(gpu, owner, window, cx));
        if presenter.place(bounds, window.scale_factor()) {
            viewport.dirty = true;
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    /// Paint: render if needed and show the frame. False when there is
    /// nothing to show (no device yet, or rendering failed): the caller
    /// paints its fallback.
    pub(crate) fn paint(
        this: &Rc<RefCell<Self>>,
        id: &GlobalElementId,
        bounds: Bounds<Pixels>,
        hitbox: &Hitbox,
        window: &mut Window,
    ) -> bool {
        let native_input = {
            let mut guard = this.borrow_mut();
            let Self {
                content,
                presenter,
                dirty,
                ..
            } = &mut *guard;
            let Some(presenter) = presenter else {
                return false;
            };
            let dirty = std::mem::take(dirty);
            let mut draw = |view: &wgpu::TextureView, size| content.draw(view, size);
            if let Err(err) = presenter.paint(id, bounds, dirty, window, &mut draw) {
                log::error!("gpu viewport: {err:#}");
                return false;
            }
            presenter.native_input()
        };
        // Over a native surface GPUI skips the bubble phase; input arrives
        // through the surface instead.
        if !native_input {
            listen_pointer(window, hitbox, this.clone());
        }
        true
    }
}

impl<C: ViewportContent> ViewportInput for Viewport<C> {
    fn pointer(&mut self, input: PointerInput) -> bool {
        let changed = match input {
            PointerInput::Press(position) => {
                self.drag_from = Some(position);
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
                None => false,
                // The release happened elsewhere.
                Some(_) if !dragging => {
                    self.drag_from = None;
                    false
                }
                Some(from) => {
                    self.drag_from = Some(position);
                    let dx = f32::from(position.x - from.x);
                    let dy = f32::from(position.y - from.y);
                    (dx != 0.0 || dy != 0.0) && self.content.drag(dx, dy, shift)
                }
            },
            PointerInput::Scroll(pixels) => self.content.scroll(pixels),
        };
        self.dirty |= changed;
        changed
    }

    #[cfg(target_os = "windows")]
    fn redraw_native(&mut self) {
        let Self {
            content,
            presenter,
            dirty,
            ..
        } = self;
        if let Some(presenter) = presenter {
            presenter.redraw_native(&mut |view, size| content.draw(view, size));
            *dirty = false;
        }
    }
}

/// The layout every viewport element asks for: full size. An auto-sized
/// custom element gets zero height in a flex container and paints nothing.
pub(crate) fn full_size_layout(window: &mut Window, cx: &mut App) -> LayoutId {
    let style = Style {
        size: gpui::Size::full(),
        ..Default::default()
    };
    window.request_layout(style, [], cx)
}

/// Device-pixel size of `bounds`, at least 1×1 and at most `max` per side.
fn device_size(bounds: Bounds<Pixels>, scale: f32, max: u32) -> (u32, u32) {
    let device: Bounds<DevicePixels> = bounds.to_device_pixels(scale);
    (
        (device.size.width.0.max(1) as u32).min(max),
        (device.size.height.0.max(1) as u32).min(max),
    )
}

/// Feeds a viewport's pointer input from ordinary GPUI listeners (the texture
/// and readback paths), asking the hosting view for a new frame when the view
/// changed. Registered during paint, as GPUI clears listeners every frame.
fn listen_pointer<C: ViewportContent>(
    window: &mut Window,
    hitbox: &Hitbox,
    viewport: Rc<RefCell<Viewport<C>>>,
) {
    let view = window.current_view();
    let feed = Rc::new(move |input: PointerInput, cx: &mut App| {
        if viewport.borrow_mut().pointer(input) {
            cx.notify(view);
        }
    });
    window.on_mouse_event({
        let (feed, hitbox) = (feed.clone(), hitbox.clone());
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble
                && event.button == MouseButton::Left
                && hitbox.is_hovered(window)
            {
                window.capture_pointer(hitbox.id);
                feed(PointerInput::Press(event.position), cx);
            }
        }
    });
    window.on_mouse_event({
        let feed = feed.clone();
        move |event: &MouseMoveEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble {
                let input = PointerInput::Move {
                    position: event.position,
                    dragging: event.dragging(),
                    shift: event.modifiers.shift,
                };
                feed(input, cx);
            }
        }
    });
    window.on_mouse_event({
        let feed = feed.clone();
        move |event: &MouseUpEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                feed(PointerInput::Release, cx);
            }
        }
    });
    let hitbox = hitbox.clone();
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
            feed(PointerInput::Scroll(scroll_pixels(event.delta)), cx);
        }
    });
}

/// One viewport's way to the screen. See the module docs.
enum Presenter {
    #[cfg(target_family = "wasm")]
    Texture(texture::TexturePresenter),
    #[cfg(target_os = "windows")]
    Native(native::NativePresenter),
    #[cfg(not(target_family = "wasm"))]
    Readback(ReadbackPresenter),
}

#[allow(unused_variables)]
impl Presenter {
    fn new(
        gpu: &Rc<Gpu>,
        owner: Weak<RefCell<dyn ViewportInput>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        #[cfg(target_family = "wasm")]
        let presenter = Self::Texture(texture::TexturePresenter::new(gpu.clone()));
        #[cfg(target_os = "windows")]
        let presenter = match native::NativePresenter::new(gpu.clone(), owner, window, cx) {
            Ok(presenter) => Self::Native(presenter),
            Err(error) => {
                log::warn!(
                    "gpu viewport: window composition unavailable, reading frames back instead: {error:#}"
                );
                Self::Readback(ReadbackPresenter::new(gpu.clone()))
            }
        };
        #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
        let presenter = Self::Readback(ReadbackPresenter::new(gpu.clone()));
        presenter
    }

    /// Whether pointer input arrives through a native surface rather than
    /// GPUI listeners.
    fn native_input(&self) -> bool {
        match self {
            #[cfg(target_os = "windows")]
            Self::Native(_) => true,
            _ => false,
        }
    }

    /// Places a native surface; true when it was resized and needs a frame.
    fn place(&mut self, bounds: Bounds<Pixels>, scale: f32) -> bool {
        match self {
            #[cfg(target_os = "windows")]
            Self::Native(presenter) => presenter.place(bounds, scale).unwrap_or_else(|error| {
                log::error!("gpu viewport: placing the native surface: {error:#}");
                false
            }),
            _ => false,
        }
    }

    /// Renders a frame if `dirty` (or the target had to change) and shows the
    /// current one.
    fn paint(
        &mut self,
        id: &GlobalElementId,
        bounds: Bounds<Pixels>,
        dirty: bool,
        window: &mut Window,
        draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32)),
    ) -> Result<()> {
        match self {
            #[cfg(target_family = "wasm")]
            Self::Texture(presenter) => presenter.paint(bounds, dirty, window, draw),
            #[cfg(target_os = "windows")]
            Self::Native(presenter) => {
                presenter.paint(id, dirty, window, draw);
                Ok(())
            }
            #[cfg(not(target_family = "wasm"))]
            Self::Readback(presenter) => presenter.paint(bounds, dirty, window, draw),
        }
    }

    /// Native surfaces render outside GPUI's frames too.
    #[cfg(target_os = "windows")]
    fn redraw_native(&mut self, draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32))) {
        if let Self::Native(presenter) = self {
            presenter.render(draw);
        }
    }
}

// ---------------------------------------------------------------------------
// Readback: offscreen render, CPU readback, `paint_image`.

/// Renders offscreen and paints the pixels as an image. Every desktop can do
/// this; it costs a GPU→CPU copy and an atlas upload per frame.
#[cfg(not(target_family = "wasm"))]
struct ReadbackPresenter {
    gpu: Rc<Gpu>,
    target: Option<workbench_render::ReadbackTarget>,
    image: Option<Arc<gpui::RenderImage>>,
}

#[cfg(not(target_family = "wasm"))]
impl ReadbackPresenter {
    fn new(gpu: Rc<Gpu>) -> Self {
        Self {
            gpu,
            target: None,
            image: None,
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        dirty: bool,
        window: &mut Window,
        draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32)),
    ) -> Result<()> {
        let gpu = &self.gpu;
        let size = device_size(bounds, window.scale_factor(), gpu.max_texture.min(MAX_OFFSCREEN));
        let resized = self.target.as_ref().is_none_or(|t| t.size() != size);
        if resized {
            self.target = Some(workbench_render::ReadbackTarget::new(
                &gpu.device,
                gpu.target_format,
                size.0,
                size.1,
            ));
        }
        if resized || dirty || self.image.is_none() {
            let target = self.target.as_ref().expect("allocated above");
            draw(target.view(), size);
            let bytes = target
                .read(&gpu.device, &gpu.queue)
                .ok_or_else(|| anyhow::anyhow!("reading the frame back failed"))?;
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

// ---------------------------------------------------------------------------
// Web: a dynamic texture on GPUI's device.

#[cfg(target_family = "wasm")]
mod texture {
    use std::rc::Rc;

    use anyhow::Result;
    use gpui::{Bounds, DevicePixels, Pixels, Window, size};
    use gpui_wgpu::WgpuDynamicTexture;

    use super::{Gpu, MAX_OFFSCREEN, device_size};

    /// Renders during paint into a texture GPUI samples as a sprite.
    pub(super) struct TexturePresenter {
        gpu: Rc<Gpu>,
        texture: Option<(WgpuDynamicTexture, (u32, u32))>,
    }

    impl TexturePresenter {
        pub(super) fn new(gpu: Rc<Gpu>) -> Self {
            Self { gpu, texture: None }
        }

        pub(super) fn paint(
            &mut self,
            bounds: Bounds<Pixels>,
            dirty: bool,
            window: &mut Window,
            draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32)),
        ) -> Result<()> {
            let max = self.gpu.max_texture.min(MAX_OFFSCREEN);
            let target = device_size(bounds, window.scale_factor(), max);
            let resized = self
                .texture
                .as_ref()
                .is_none_or(|(_, allocated)| *allocated != target);
            if resized {
                let texture = self.gpu.context.create_dynamic_texture(size(
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
    use std::rc::{Rc, Weak};
    use std::time::{Duration, Instant};

    use anyhow::{Context as _, Result, anyhow};
    use gpui::{
        App, BackgroundExecutor, Bounds, DevicePixels, DispatchEventResult, ForegroundExecutor,
        GlobalElementId, MouseButton, Pixels, PlatformInput, Point, Window,
        WindowCompositionSurface, WindowId,
    };
    use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;
    use windows::core::{IUnknown, Interface as _};

    use super::{Gpu, PointerInput, ViewportInput, scroll_pixels};

    /// Input-driven frames are paced to about this interval. A mouse posts
    /// events far faster than a display shows frames, and Mailbox presents
    /// never block to slow that down.
    const FRAME_INTERVAL: Duration = Duration::from_micros(8_333);

    thread_local! {
        /// Native surfaces of closed viewports, by window. gpui-fast cannot
        /// remove a surface on Windows (`remove_surface` hides it first, and
        /// hiding casts the visual to `IDCompositionVisual3`, which visuals
        /// of its `DCompositionCreateDevice` device don't implement:
        /// E_NOINTERFACE), so they are parked off screen and reused.
        static SPARE: RefCell<Vec<(WindowId, WindowCompositionSurface)>> =
            const { RefCell::new(Vec::new()) };
    }

    /// Whether this thread's locals are being torn down: at process exit the
    /// application can be dropped by thread-local destructors. Those run in
    /// reverse order of registration, and [`SPARE`] registers only when the
    /// first native surface is made — after every thread local that can own
    /// one — so it is gone by then. There is no window left to hide anything
    /// in, and touching a destroyed thread local would abort the process.
    fn tearing_down() -> bool {
        SPARE.try_with(|_| ()).is_err()
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
        /// Whether the surface has its bounds (rather than empty ones).
        shown: Cell<bool>,
        /// Liveness tokens held by frames that include the element.
        live: Cell<usize>,
        /// A redraw task is queued.
        redraw_queued: Cell<bool>,
        last_present: Cell<Instant>,
        owner: Weak<RefCell<dyn ViewportInput>>,
        executor: ForegroundExecutor,
        timers: BackgroundExecutor,
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
                    log::error!("gpu viewport: hiding the native surface: {error:#}");
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
                log::error!("gpu viewport: committing the composition: {error:#}");
            }
        }

        /// Renders the owner's next frame, at most one per [`FRAME_INTERVAL`].
        fn queue_redraw(self: &Rc<Self>) {
            if self.redraw_queued.replace(true) {
                return;
            }
            let wait = FRAME_INTERVAL.saturating_sub(self.last_present.get().elapsed());
            let timer = self.timers.timer(wait);
            let shared = Rc::downgrade(self);
            self.executor
                .spawn(async move {
                    timer.await;
                    let Some(shared) = shared.upgrade() else {
                        return;
                    };
                    shared.redraw_queued.set(false);
                    if let Some(owner) = shared.owner.upgrade() {
                        owner.borrow_mut().redraw_native();
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
            self.last_present.set(Instant::now());
            if self.needs_commit.replace(false) {
                self.commit();
            }
        }
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

    /// Kept as the painted element's state, which GPUI keeps for exactly as
    /// long as a frame includes the element: a frame that reuses a retained
    /// view carries it along, and the frame that replaces the last one
    /// including the element drops it. When the last token goes, the element
    /// is gone from the screen (its tab was switched away, its panel closed or
    /// collapsed) and the surface, which GPUI would otherwise leave where it
    /// was, is hidden.
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
                // Dropped while GPUI finishes a frame: act once it is done.
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

    pub(super) struct NativePresenter {
        shared: Rc<Shared>,
    }

    impl NativePresenter {
        pub(super) fn new(
            gpu: Rc<Gpu>,
            owner: Weak<RefCell<dyn ViewportInput>>,
            window: &mut Window,
            cx: &mut App,
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

            // Pointer input over the surface comes here rather than to GPUI
            // listeners, in window logical pixels.
            let input_owner = owner.clone();
            let shared = Rc::new(Shared {
                gpu,
                window: window_id,
                surface: surface.clone(),
                swap_chain,
                config: RefCell::new(config),
                needs_commit: Cell::new(true),
                shown: Cell::new(false),
                live: Cell::new(0),
                redraw_queued: Cell::new(false),
                last_present: Cell::new(Instant::now()),
                owner,
                executor: cx.foreground_executor().clone(),
                timers: cx.background_executor().clone(),
            });
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
                let changed = input_owner
                    .upgrade()
                    .is_some_and(|owner| owner.borrow_mut().pointer(input));
                if changed {
                    if let Some(shared) = weak.upgrade() {
                        shared.queue_redraw();
                    }
                }
                DispatchEventResult {
                    propagate: false,
                    default_prevented: false,
                }
            })?;

            Ok(Self { shared })
        }

        /// Puts the surface where GPUI laid the element out, resizing the swap
        /// chain to match. True when it was resized.
        pub(super) fn place(&self, bounds: Bounds<Pixels>, scale: f32) -> Result<bool> {
            let shared = &self.shared;
            let device: Bounds<DevicePixels> = bounds.to_device_pixels(scale);
            if device.size.width.0 <= 0 || device.size.height.0 <= 0 {
                shared.hide();
                return Ok(false);
            }
            shared.surface.platform_surface()?.set_bounds(device)?;
            shared.shown.set(true);

            let max = shared.gpu.max_texture;
            let size = (
                (device.size.width.0 as u32).min(max),
                (device.size.height.0 as u32).min(max),
            );
            let mut config = shared.config.borrow_mut();
            if (config.width, config.height) == size {
                return Ok(false);
            }
            (config.width, config.height) = size;
            shared.swap_chain.configure(&shared.gpu.device, &config);
            shared.needs_commit.set(true);
            Ok(true)
        }

        pub(super) fn paint(
            &self,
            id: &GlobalElementId,
            dirty: bool,
            window: &mut Window,
            draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32)),
        ) {
            let shared = &self.shared;
            window.with_element_state(id, |token: Option<LiveToken>, _| {
                let token = token
                    .filter(|token| Rc::ptr_eq(&token.0, shared))
                    .unwrap_or_else(|| LiveToken::new(shared));
                ((), token)
            });
            if dirty {
                shared.render(draw);
            }
        }

        pub(super) fn render(&self, draw: &mut dyn FnMut(&wgpu::TextureView, (u32, u32))) {
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
