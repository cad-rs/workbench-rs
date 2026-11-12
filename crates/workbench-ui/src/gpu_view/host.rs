//! Generic viewport host Element (the renderer host contract).
//!
//! Absorbs the whole shell the product side used to duplicate (moved in from mulphy's gpu3d.rs;
//! the two copies of the shell had drifted apart: a zero-height layout bug fixed separately in
//! each kept a "surface sharing is broken" misdiagnosis alive for 7 rounds, and hit testing
//! existed only on the product side; since then the framework has the only implementation).
//! Full-size layout, raw input forwarding and the presentation path all live in the shared
//! [`gpui_gpu_view::Viewport`]; this module only maps input onto the render core's interactive
//! camera and, at paint time, draws the product's scene with the render core. Without an adapter
//! it shows the dark background as is.
//!
//! Layering: render semantics (camera sensitivity / geometry / shading / line width) belong to
//! [`workbench_render`]; this module only passes on raw input and target parameters. Each frame
//! the product builds a [`workbench_render::RenderScene`] (world-space geometry + the **base**
//! camera + background) and the base view [`RenderViewportElement::base_view`]; the render core's
//! session state layers the interactive camera offset on top at render time.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Bounds, Element, ElementId, GlobalElementId, HitboxBehavior, InspectorElementId, LayoutId,
    Pixels, Window,
};
use gpui_gpu_view::{Gpu, Viewport, ViewportContent, ViewportInput};
use workbench_render as wbr;

/// Generic render viewport: the product passes the scene (base camera) + base view, and the render
/// core layers the interactive offset on top. For the presentation path see the shared
/// [`gpui_gpu_view::Viewport`]; with no usable adapter it shows the dark background as is.
///
/// There is only one host per process (the renderer and the presentation target are singletons,
/// as before the migration): two of these elements shown at once contend for the same target.
pub struct RenderViewportElement {
    /// Rebuilt every frame: world-space geometry + the base camera
    /// (`scene_camera`-style semantics) + background.
    pub scene: wbr::RenderScene,
    /// The base view (yaw, pitch, in degrees) matching `scene.camera`; the interactive offset is
    /// layered on top of it.
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
    /// Where the pointer was last seen with the button held, for drag deltas.
    last_pointer: Option<gpui::Point<Pixels>>,
}

impl ViewportContent for HostScene {
    // Raw input forwarding: drag offset (px) + the Shift key state, wheel pixels → zoom factor;
    // the sensitivity / conversion / clamp semantics all live in the render core
    // (`wbr::CamOffset`).
    fn input(&mut self, input: ViewportInput) -> bool {
        if let Some(pixels) = input.scroll {
            wbr::cam_zoom((-pixels as f64 * 0.0012).exp());
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
                } else if input.shift {
                    wbr::cam_pan(dx as f64, dy as f64);
                    true
                } else {
                    wbr::cam_orbit(dx as f64, dy as f64);
                    true
                }
            }
            None => false,
        };
        self.last_pointer = Some(position);
        changed
    }

    fn draw(&mut self, _gpu: &Gpu, view: &wgpu::TextureView, size: (u32, u32)) {
        let (Some(renderer), Some((scene, base_camera, base_view))) =
            (self.renderer.as_mut(), self.scene.as_mut())
        else {
            return;
        };
        // Base camera (built by the product) → interactive camera layered on top (semantics in the
        // render core).
        let mut camera = *base_camera;
        wbr::cam_apply(base_view.0, base_view.1, &mut camera, size.1 as f64);
        scene.camera = camera;
        renderer.render(scene, size.0, size.1, view);
    }
}

thread_local! {
    // Singleton on the gpui UI thread; the device comes from the framework's shared slot
    // `shared_gpu`.
    static HOST: Rc<RefCell<Viewport<HostScene>>> = Rc::new(RefCell::new(Viewport::new(
        HostScene {
            renderer: None,
            scene: None,
            last_pointer: None,
        },
    )));
}

impl gpui::IntoElement for RenderViewportElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
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
        (gpui_gpu_view::full_size_layout(window, cx), ())
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
        // The framework's shared GPU slot; when it is unavailable (no adapter), paint shows the
        // dark background as is.
        let gpu = gpui_gpu_view::shared_gpu(window);
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
            // No GPU adapter / render failure: show the dark background as is (diagnostics go to
            // the log).
            window.paint_quad(gpui::fill(bounds, gpui::rgb(0x1A1D24)));
        }
    }
}
