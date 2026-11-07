//! workbench-render render core tests (headless: the GPU pipelines run without a window;
//! environments with no usable adapter skip silently). The host passes in the render core's
//! device (the production contract): the tests build a local device and assert on the pixels of
//! the bitmap `render_readback` reads back.

use workbench_render::{
    CamOffset, LineVertexGpu, OrthoCamera, RenderScene, RenderVertex, Renderer,
    cam_offset_for_test, reset_cam_offset,
};

/// Builds a local headless device + renderer (equivalent to the production contract where the
/// shell passes in the shared device).
fn headless_renderer() -> Option<Renderer> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wb-render-test"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: Default::default(),
        trace: wgpu::Trace::Off,
        default_queue: wgpu::QueueDescriptor::default(),
    }))
    .ok()?;
    Some(Renderer::new(device, queue, wgpu::TextureFormat::Bgra8Unorm))
}

fn cam() -> OrthoCamera {
    // Top-down (camera above, on +z): facing the geometry under test in the x-y plane.
    OrthoCamera {
        yaw_deg: 0.0,
        pitch_deg: 90.0,
        center: [0.0, 0.0, 0.0],
        radius: 1.0,
    }
}

/// A red triangle fills the viewport center: the center pixel is red, the corners background;
/// size checked, padding removed.
#[test]
fn renders_triangle_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    let red = [1.0, 0.0, 0.0, 1.0];
    let v = |x: f32, y: f32| RenderVertex {
        pos: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: red,
    };
    let scene = RenderScene {
        // Medium triangle (NDC ±0.42): the center inside, the four corners outside.
        tris: vec![v(-0.5, -0.5), v(0.5, -0.5), v(0.0, 0.5)],
        tris_alpha: vec![],
        sort_alpha: false,
        volume: None,
        lines: vec![],
        camera: cam(),
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.5,
    };
    let (w, h) = (64u32, 64u32);
    let bytes = r.render_readback(&scene, w, h).expect("render");
    assert_eq!(bytes.len(), (w * h * 4) as usize, "no row padding");
    // readback is BGRA (texture Bgra8Unorm, matching gpui's RenderImage).
    let px = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        (bytes[i + 2], bytes[i + 1], bytes[i]) // (R, G, B)
    };
    let (r_, g, b) = px(32, 32);
    // Headlight shading: the normal faces the camera → shade ≈ 1.0, red nearly full.
    assert!(r_ > 150, "center red, got {r_} {g} {b}");
    assert!(g < 60 && b < 60, "center not green/blue");
    // The corner is outside the triangle (vertex (0,2) only reaches the middle of the top edge) →
    // background.
    let (br, bg, bb) = px(2, 2);
    assert!(br < 30 && bg < 30 && bb < 30, "corner bg, got {br} {bg} {bb}");
    // MSAA 4x: the slanted edges must have partly covered pixels (red blended with the
    // background), so anti-aliasing is in effect.
    let mixed = (0..(w * h) as usize)
        .filter(|&i| {
            let r = bytes[i * 4 + 2];
            r > 1 && r < 254 && bytes[i * 4 + 1] < 60
        })
        .count();
    assert!(mixed > 0, "MSAA blends edge pixels, got {mixed} mixed");
}

/// Line pipeline: under the pitch=90 top-down view an x-axis segment → a vertical screen line
/// (right=+y → x_screen stays on the middle column; the line length ±1.5 exceeds the half-height
/// 1.2 → it spans the whole screen): column 32 is green on every row, column 2 has no green.
/// The 1.5px expanded quad covers columns 31/32 at 75% each (g≈191 after the MSAA resolve).
#[test]
fn renders_line_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    let lv = |x: f32| LineVertexGpu {
        pos: [x, 0.0, 0.0],
        color: [0.0, 1.0, 0.0, 1.0],
    };
    let scene = RenderScene {
        tris: vec![],
        tris_alpha: vec![],
        sort_alpha: false,
        volume: None,
        lines: vec![lv(-1.5), lv(1.5)],
        camera: cam(),
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.5,
    };
    let (w, h) = (64u32, 64u32);
    let bytes = r.render_readback(&scene, w, h).expect("render");
    // Number of green pixels in column c (the vertical line check).
    let green_in_column = |c: usize| -> usize {
        (0..h as usize)
            .filter(|&y| bytes[(y * w as usize + c) * 4 + 1] > 100)
            .count()
    };
    // The vertical line is at the viewport center (NDC x=0 falls exactly on the pixel 31/32
    // boundary, so 1.5px wide spans both columns at 75% each); the side columns are zero.
    let n_mid = green_in_column(31) + green_in_column(32);
    assert!(n_mid > 30, "center column has the line, got {n_mid}");
    assert_eq!(green_in_column(2), 0, "side column stays background");
}

/// Screen-space line width: an 8px line quad keeps a physical thickness of 8 columns in both the
/// 64px and the 128px viewport (px semantics rather than NDC stretching: doubling the resolution
/// leaves the thickness unchanged).
#[test]
fn line_screen_space_width_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    let scene = |width: f32| RenderScene {
        tris: vec![],
        tris_alpha: vec![],
        sort_alpha: false,
        volume: None,
        lines: vec![
            LineVertexGpu { pos: [-1.5, 0.0, 0.0], color: [0.0, 1.0, 0.0, 1.0] },
            LineVertexGpu { pos: [1.5, 0.0, 0.0], color: [0.0, 1.0, 0.0, 1.0] },
        ],
        camera: cam(),
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: width,
    };
    // 64px: NDC x=0 is on the pixel 31/32 boundary, 8px wide → columns 28..35 covered.
    let bytes = r.render_readback(&scene(8.0), 64, 64).expect("render 64");
    let col64 = |bytes: &[u8], x: usize| {
        (0..64usize)
            .filter(|&y| bytes[(y * 64 + x) * 4 + 1] > 100)
            .count()
    };
    for c in [30usize, 33] {
        assert!(col64(&bytes, c) > 50, "64px: col {c} inside thick line");
    }
    assert_eq!(col64(&bytes, 24), 0, "64px: outside band background");
    // 128px: the center is on the 63/64 boundary, still 8 physical pixels → columns 60..67.
    let bytes = r.render_readback(&scene(8.0), 128, 128).expect("render 128");
    let col128 = |x: usize| {
        (0..128usize)
            .filter(|&y| bytes[(y * 128 + x) * 4 + 1] > 100)
            .count()
    };
    for c in [62usize, 65] {
        assert!(col128(c) > 100, "128px: col {c} inside thick line");
    }
    assert_eq!(col128(50), 0, "128px: outside band background");
}

/// Line depth correctness: quad corner depths are interpolated linearly by the segment parameter.
/// An opaque wall hides what is behind it; a line slanting across the depth boundary is "hidden
/// behind the wall, shown in front of it" (wrong corner interpolation misplaces the boundary).
#[test]
fn line_depth_occlusion_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    // "Front" view (yaw 0 pitch 0): looking along -x, screen right = +y, depth = world x
    // (larger is closer to the camera).
    let cam = OrthoCamera {
        yaw_deg: 0.0,
        pitch_deg: 0.0,
        center: [0.0, 0.0, 0.0],
        radius: 1.0,
    };
    let rv = |x: f32, y: f32, z: f32| RenderVertex {
        pos: [x, y, z],
        normal: [1.0, 0.0, 0.0],
        color: [1.0, 0.0, 0.0, 1.0],
    };
    // A red wall covering the whole viewport (±3 covers NDC ±1.2), at the given depth x.
    let wall = |x: f32| vec![rv(x, -3.0, -3.0), rv(x, 3.0, -3.0), rv(x, 0.0, 3.0)];
    let lv = |x: f32, y: f32| LineVertexGpu {
        pos: [x, y, 0.0],
        color: [0.0, 1.0, 0.0, 1.0],
    };
    let greens = |bytes: &[u8]| {
        (0..(64 * 64) as usize)
            .filter(|&i| bytes[i * 4 + 1] > 100)
            .count()
    };
    // Scene A: the line is entirely behind the wall (x=-0.5, always < 0) → no green in the whole
    // frame.
    let behind = RenderScene {
        tris_alpha: vec![],
        sort_alpha: false,
        volume: None,
        tris: wall(0.0),
        lines: vec![lv(-0.5, -1.5), lv(-0.5, 1.5)],
        camera: cam,
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 4.0,
    };
    let bytes = r.render_readback(&behind, 64, 64).expect("render behind");
    assert_eq!(greens(&bytes), 0, "line fully behind the wall is occluded");
    // Scene B: the line slants through depth (the t=0 end at x=-1 behind the wall, the t=1 end at
    // x=+1 in front), wall at x=0.3 → the depth boundary is at segment parameter 0.65 (column
    // ≈44); right of it is visible, left of it hidden by the wall. If the corner depths weren't
    // interpolated by t (the whole segment taking an endpoint's depth), the boundary would slip
    // to fully hidden.
    let crossing = RenderScene {
        tris_alpha: vec![],
        sort_alpha: false,
        volume: None,
        tris: wall(0.3),
        lines: vec![lv(-1.0, -1.5), lv(1.0, 1.5)],
        camera: cam,
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 4.0,
    };
    let bytes = r.render_readback(&crossing, 64, 64).expect("render crossing");
    let col = |x: usize| {
        (0..64usize)
            .filter(|&y| bytes[(y * 64 + x) * 4 + 1] > 100)
            .count()
    };
    assert_eq!(col(10), 0, "behind-wall part occluded");
    assert!(col(52) > 0, "in-front part visible through wall zone");
    // The line is also visible at the right edge, which the wall doesn't cover (column 61+,
    // NDC x > 0.833).
    assert!(col(62) > 0, "beyond wall edge visible");
}

/// Interactive camera offset: orbit accumulation (degrees/pixel), zoom multiplier clamp, reset.
#[test]
fn cam_offset_semantics() {
    let mut c = CamOffset::default();
    assert_eq!((c.yaw, c.pitch, c.zoom), (0.0, 0.0, 1.0));
    c.orbit_by(100.0, -50.0);
    assert_eq!((c.yaw, c.pitch), (40.0, -20.0), "0.4°/px");
    c.zoom_by(0.5);
    c.zoom_by(0.5);
    c.zoom_by(0.5);
    c.zoom_by(0.5); // 1.0 × 0.5⁴ = 0.0625 → clamp 0.15
    assert!((c.zoom - 0.15).abs() < 1e-12, "zoom floor");
    c.zoom_by(8.0);
    c.zoom_by(8.0); // 0.15 × 64 = 9.6 → clamp 6.0
    assert!((c.zoom - 6.0).abs() < 1e-12, "zoom ceiling");
    // Reset semantics (the view preset buttons).
    reset_cam_offset();
    assert_eq!(cam_offset_for_test(), CamOffset::default());
    // Pan accumulation and clamp (±4000px so the view is never lost).
    let mut c = CamOffset::default();
    c.pan_by(100.0, -200.0);
    assert_eq!(c.pan, [100.0, -200.0]);
    c.pan_by(9000.0, -9000.0);
    assert_eq!(c.pan, [4000.0, -4000.0], "pan clamps");
}

/// Adapter diagnostics are available (None without a GPU, no panic).
#[test]
fn adapter_info_does_not_panic() {
    let _ = workbench_render::adapter_info();
}

/// Transparent-bucket centroid sort (round-206 P1, the declarative `sort_alpha` switch): ① switch
/// off = P0 semantics, array order = blend order, so a near face declared first is dominated by
/// the far face (encoding the measured in-group artifact: center b=128 ≈ twice the near green
/// g=64, theory (64,64,128)); ② switch on = centroid far-to-near reorder, the near face drawn
/// later dominates ((63,128,64)); ③ swapping the declaration order gives byte-identical output:
/// sorting removes declaration-order semantics (a strong correctness assertion for sorting two
/// convex triangles).
/// Front view (yaw 0 pitch 0): depth = world x, larger is closer to the camera.
#[test]
fn alpha_centroid_sort_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    let cam = OrthoCamera {
        yaw_deg: 0.0,
        pitch_deg: 0.0,
        center: [0.0, 0.0, 0.0],
        radius: 1.0,
    };
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN05: [f32; 4] = [0.0, 1.0, 0.0, 0.5];
    const BLUE05: [f32; 4] = [0.0, 0.0, 1.0, 0.5];
    let rv = |x: f32, y: f32, z: f32, c: [f32; 4]| RenderVertex {
        pos: [x, y, z],
        normal: [1.0, 0.0, 0.0],
        color: c,
    };
    let wall = |x: f32| vec![rv(x, -3.0, -3.0, RED), rv(x, 3.0, -3.0, RED), rv(x, 0.0, 3.0, RED)];
    let tq = |x: f32, c: [f32; 4]| {
        vec![rv(x, -0.5, -0.5, c), rv(x, 0.5, -0.5, c), rv(x, 0.0, 0.5, c)]
    };
    let px = |bytes: &[u8]| {
        let i = ((32 * 64 + 32) * 4) as usize;
        (bytes[i + 2], bytes[i + 1], bytes[i])
    };
    // Near green (x=0.5) declared first, far blue (x=0.25) second, vs swapped. The red wall at x=0
    // is behind both transparent faces (read-only depth doesn't cull the blending in front of it).
    let near_first = [tq(0.5, GREEN05), tq(0.25, BLUE05)].concat();
    let far_first = [tq(0.25, BLUE05), tq(0.5, GREEN05)].concat();
    let base = |tris_alpha: Vec<RenderVertex>, sort_alpha: bool| RenderScene {
        tris: wall(0.0),
        tris_alpha,
        sort_alpha,
        volume: None,
        lines: vec![],
        camera: cam,
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.0,
    };
    // ① Switch off: far blue dominates (the framework-level encoding of the P0 in-group artifact).
    let bytes_a = r
        .render_readback(&base(near_first.clone(), false), 64, 64)
        .expect("render off");
    let (ar, ag, ab) = px(&bytes_a);
    assert!(
        ab > 100 && ab > ag,
        "off: later-declared far blue dominates: {ar} {ag} {ab}"
    );
    // ② Switch on (same declaration order): after the reorder, near green is drawn later and
    // dominates.
    let bytes_b = r
        .render_readback(&base(near_first.clone(), true), 64, 64)
        .expect("render on");
    let (br, bg, bb) = px(&bytes_b);
    assert!(
        bg > 100 && bg > bb,
        "on: near green dominates after sort: {br} {bg} {bb}"
    );
    // ③ Swapped declaration order + sort on: byte-identical output (the same final vertex order).
    let bytes_c = r
        .render_readback(&base(far_first, true), 64, 64)
        .expect("render on swapped");
    assert_eq!(bytes_b, bytes_c, "sorted output independent of declaration order");
}

/// Pure logic of the centroid sort (no GPU; runs without an adapter too): reorders by descending
/// distance, stable for equal distances (keeps declaration order), returns None with the switch
/// off / for a single triangle. The "front" view camera's eye is at center + (3r, 0, 0).
#[test]
fn alpha_centroid_sort_helper_pure() {
    let v = |x: f32, tag: f32| RenderVertex {
        pos: [x, 0.0, 0.0],
        normal: [1.0, 0.0, 0.0],
        color: [tag, 0.0, 0.0, 0.5],
    };
    let cam = OrthoCamera {
        yaw_deg: 0.0,
        pitch_deg: 0.0,
        center: [0.0, 0.0, 0.0],
        radius: 1.0,
    };
    let mut scene = RenderScene {
        tris: vec![],
        // Centroids x = 1 / 3 / 2 → eye at (3,0,0), ascending distance = descending x → farthest
        // drawn first.
        tris_alpha: vec![
            v(1.0, 0.1), v(1.0, 0.1), v(1.0, 0.1), // t0: farthest
            v(3.0, 0.3), v(3.0, 0.3), v(3.0, 0.3), // t1: nearest
            v(2.0, 0.2), v(2.0, 0.2), v(2.0, 0.2), // t2: middle
        ],
        sort_alpha: true,
        volume: None,
        lines: vec![],
        camera: cam,
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.0,
    };
    let sorted = scene.alpha_sorted().expect("sorted");
    assert_eq!(sorted[0].pos[0], 1.0, "farthest first");
    assert_eq!(sorted[3].pos[0], 2.0, "middle second");
    assert_eq!(sorted[6].pos[0], 3.0, "nearest last");
    // Stable for equal distances: the whole group at the same x → declaration order kept (tags
    // 0.1..0.6 in their original order).
    scene.tris_alpha = (0..6).map(|k| v(1.0, 0.1 + k as f32 * 0.1)).collect();
    let sorted = scene.alpha_sorted().unwrap();
    for (k, vtx) in sorted.iter().enumerate() {
        let expect = 0.1 + k as f32 * 0.1;
        assert!((vtx.color[0] - expect).abs() < 1e-6, "stable order at {k}");
    }
    // Switch off → None; a single triangle → None (nothing to order).
    scene.sort_alpha = false;
    assert!(scene.alpha_sorted().is_none());
    scene.sort_alpha = true;
    scene.tris_alpha.truncate(3);
    assert!(scene.alpha_sorted().is_none());
}


/// Transparent bucket (round-205 P0): ALPHA_BLENDING + depth write OFF. ① A transparent face in
/// front of the wall blends normally (0.5 green over the red wall → r≈g); ② a transparent face
/// behind the wall is culled by read-only depth (the pixel keeps the wall color); ③ two
/// transparent faces on screen blend in declaration order (the later-declared one dominates the
/// overlap). Front view (yaw 0 pitch 0): depth = world x, larger is closer to the camera.
#[test]
fn transparent_tris_blend_order_and_depth() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    let cam = OrthoCamera {
        yaw_deg: 0.0,
        pitch_deg: 0.0,
        center: [0.0, 0.0, 0.0],
        radius: 1.0,
    };
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN05: [f32; 4] = [0.0, 1.0, 0.0, 0.5];
    const BLUE05: [f32; 4] = [0.0, 0.0, 1.0, 0.5];
    let rv = |x: f32, y: f32, z: f32, c: [f32; 4]| RenderVertex {
        pos: [x, y, z],
        normal: [1.0, 0.0, 0.0],
        color: c,
    };
    // The red wall covers the whole viewport (±3 covers NDC ±1.2); the transparent faces cover the
    // viewport center.
    let wall = |x: f32| vec![rv(x, -3.0, -3.0, RED), rv(x, 3.0, -3.0, RED), rv(x, 0.0, 3.0, RED)];
    let tq = |x: f32, c: [f32; 4]| {
        vec![rv(x, -0.5, -0.5, c), rv(x, 0.5, -0.5, c), rv(x, 0.0, 0.5, c)]
    };
    // BGRA center pixel → (r, g, b).
    let px = |bytes: &[u8]| {
        let i = ((32 * 64 + 32) * 4) as usize;
        (bytes[i + 2], bytes[i + 1], bytes[i])
    };
    // ① Wall at x=0 + green 0.5 in front at x=0.5 → center = 0.5 green + 0.5 red = (127,127,0).
    let s1 = RenderScene {
        tris: wall(0.0),
        tris_alpha: tq(0.5, GREEN05),
        sort_alpha: false,
        volume: None,
        lines: vec![],
        camera: cam,
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.0,
    };
    let bytes = r.render_readback(&s1, 64, 64).expect("render blend");
    let (pr, pg, pb) = px(&bytes);
    assert!(
        (pr as i32 - 127).abs() <= 25 && (pg as i32 - 127).abs() <= 25 && pb < 40,
        "0.5 green over red wall: {pr} {pg} {pb}"
    );
    // ② Wall at x=0 + blue 0.5 behind at x=-0.5 → culled by read-only depth, the center stays
    // wall red.
    let s2 = RenderScene {
        tris: wall(0.0),
        tris_alpha: tq(-0.5, BLUE05),
        sort_alpha: false,
        volume: None,
        lines: vec![],
        camera: cam,
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.0,
    };
    let bytes = r.render_readback(&s2, 64, 64).expect("render behind");
    let (pr, pg, pb) = px(&bytes);
    assert!(pr > 200 && pg < 40 && pb < 40, "behind-wall culled: {pr} {pg} {pb}");
    // ③ Declaration order: green 0.5 (x=0.5) first, blue 0.5 (x=0.25) second → center =
    // 0.5 blue + 0.5(0.5 green + 0.5 red) = (63,63,127), blue dominates (depth writes off,
    // blend order = array order; both are in front of the wall and pass read-only depth).
    let s3 = RenderScene {
        tris: wall(0.0),
        tris_alpha: [tq(0.5, GREEN05), tq(0.25, BLUE05)].concat(),
        sort_alpha: false,
        volume: None,
        lines: vec![],
        camera: cam,
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.0,
    };
    let bytes = r.render_readback(&s3, 64, 64).expect("render order");
    let (pr, pg, pb) = px(&bytes);
    assert!(
        pb > 100 && pb > pr && pb > pg,
        "declaration order: later blue dominates: {pr} {pg} {pb}"
    );
}

// ---- Volume rendering (round-211 P2 MVP, ask doc §8 assertions ①②③) ----

/// Builds a uniform VolumeSlice (dims 8³, configurable LUT).
fn uniform_volume(value: f32, lut: Vec<[u8; 4]>) -> workbench_render::VolumeSlice {
    workbench_render::VolumeSlice {
        data: std::sync::Arc::from(vec![value; 8 * 8 * 8]),
        dims: [8, 8, 8],
        bounds: ([-0.3, -0.3, -0.3], [0.3, 0.3, 0.3]),
        lut,
        steps: 64,
        alpha_scale: 1.0,
        generation: 1,
    }
}

/// Center ray integration (ask doc §8 ①): a uniform 0.5 volume with a constant-opacity LUT
/// (8/255) and 64 steps; the center pixel's analytic value is acc_a = 1-(1-8/255)^64 ≈ 0.8696;
/// the premultiplied output over black ≈ LUT color × acc_a (±2/255), and the corner rays miss the
/// AABB and keep the background.
#[test]
fn volume_center_ray_integration_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    // LUT: green at every level, a=8/255 (constant-opacity MVP).
    let lut = vec![[0u8, 255u8, 0u8, 8u8]; 256];
    let scene = RenderScene {
        tris: vec![],
        tris_alpha: vec![],
        sort_alpha: false,
        volume: Some(uniform_volume(0.5, lut)),
        lines: vec![],
        camera: cam(),
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.0,
    };
    let bytes = r.render_readback(&scene, 64, 64).expect("render volume");
    let px = |x: u32, y: u32| {
        let i = ((y * 64 + x) * 4) as usize;
        (bytes[i + 2], bytes[i + 1], bytes[i])
    };
    // Analytic: a = 8/255, acc_a = 1-(1-a)^64, out.g = 255·acc_a (over black).
    let a = 8.0f32 / 255.0;
    let acc_a = 1.0 - (1.0 - a).powi(64);
    let expect_g = (255.0 * acc_a).round() as i32;
    let (cr, cg, cb) = px(32, 32);
    assert!(
        (cg as i32 - expect_g).abs() <= 2 && cr <= 2 && cb <= 2,
        "center ray integral: got ({cr},{cg},{cb}) expect (0,{expect_g},0)"
    );
    // The corner rays miss the AABB → background.
    let (br, bg_, bb) = px(2, 2);
    assert!(br < 4 && bg_ < 4 && bb < 4, "corner stays bg: ({br},{bg_},{bb})");
}

/// Cutoff at an opaque wall (ask doc §8 ②, normalized step semantics): the steps divide
/// [t_entry, t_wall] evenly (§4.2), so the cutoff doesn't reduce the step count but concentrates
/// the samples in front of the wall. With a "front half 0 / back half 1" field + a two-color LUT
/// (0 → green, 1 → blue): with the cutoff every sample lands in the front green half (b≈0);
/// without it the back blue half mixes in (significant b). The red wall shows through
/// (r = (1-acc_a)·255).
#[test]
fn volume_wall_truncation_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    // LUT[0] = green a=8; LUT[255] = blue a=8; the levels in between are transparent black (the
    // interface band produces no fog).
    let mut lut = vec![[0u8, 0u8, 0u8, 0u8]; 256];
    lut[0] = [0u8, 255u8, 0u8, 8u8];
    lut[255] = [0u8, 0u8, 255u8, 8u8];
    let rv = |x: f32, y: f32, z: f32| RenderVertex {
        pos: [x, y, z],
        normal: [1.0, 0.0, 0.0],
        color: [1.0, 0.0, 0.0, 1.0],
    };
    // Front-view camera (depth axis = world x): the wall at x=0 cuts across the middle of the
    // volume [-0.3,0.3]; under the top-down cam() this wall would be edge-on to the camera (face
    // normal ⊥ view ray) and wouldn't trigger the wall hit.
    let cam = OrthoCamera {
        yaw_deg: 0.0,
        pitch_deg: 0.0,
        center: [0.0, 0.0, 0.0],
        radius: 1.0,
    };
    // Field: cell centers with x < 0 (back half, far side) are 1.0 (blue); x ≥ 0 (front half,
    // near side) 0.0 (green).
    let mut data = vec![0f32; 8 * 8 * 8];
    for k in 0..8u32 {
        for j in 0..8u32 {
            for i in 0..8u32 {
                let x = -0.3 + (i as f64 + 0.5) * 0.6 / 8.0;
                if x < 0.0 {
                    data[(i + j * 8 + k * 64) as usize] = 1.0;
                }
            }
        }
    }
    let scene = RenderScene {
        tris: vec![rv(0.0, -3.0, -3.0), rv(0.0, 3.0, -3.0), rv(0.0, 0.0, 3.0)],
        tris_alpha: vec![],
        sort_alpha: false,
        volume: Some(workbench_render::VolumeSlice {
            data: std::sync::Arc::from(data),
            dims: [8, 8, 8],
            bounds: ([-0.3, -0.3, -0.3], [0.3, 0.3, 0.3]),
            lut,
            steps: 64,
            alpha_scale: 1.0,
            generation: 1,
        }),
        lines: vec![],
        camera: cam,
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.0,
    };
    let bytes = r.render_readback(&scene, 64, 64).expect("render wall+vol");
    let i = ((32 * 64 + 32) * 4) as usize;
    let (cr, cg, cb) = (bytes[i + 2], bytes[i + 1], bytes[i]);
    // Cutoff: every sample lands in the green zone (front half) → red showing through + green fog,
    // zero blue. r/g are relaxed for acc_a ∈ [all 64 samples, 7 samples fewer] (linear filtering
    // in the interface band makes the transition levels transparent).
    assert!(
        (30..=60).contains(&cr) && cg > 195 && cb <= 6,
        "wall truncation: got ({cr},{cg},{cb}) expect (30..60,>195,<=6)"
    );
}

/// LUT endpoint (ask doc §8 ③): a uniform 1.0 volume → samples LUT[255]; end-to-end evidence that
/// normalization carries through (the rgb endpoint color × the same acc_a as in ①).
#[test]
fn volume_lut_endpoint_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: no usable wgpu adapter");
        return;
    };
    let mut lut = vec![[0u8, 0u8, 0u8, 8u8]; 256];
    lut[255] = [255u8, 0u8, 0u8, 8u8]; // endpoint red (the rest black: sampling only hits 1.0)
    let scene = RenderScene {
        tris: vec![],
        tris_alpha: vec![],
        sort_alpha: false,
        volume: Some(uniform_volume(1.0, lut)),
        lines: vec![],
        camera: cam(),
        bg: [0.0, 0.0, 0.0, 1.0],
        line_width_px: 1.0,
    };
    let bytes = r.render_readback(&scene, 64, 64).expect("render lut end");
    let i = ((32 * 64 + 32) * 4) as usize;
    let (cr, cg, cb) = (bytes[i + 2], bytes[i + 1], bytes[i]);
    let a = 8.0f32 / 255.0;
    let acc_a = 1.0 - (1.0 - a).powi(64);
    let expect_r = (255.0 * acc_a).round() as i32;
    assert!(
        (cr as i32 - expect_r).abs() <= 2 && cg <= 2 && cb <= 2,
        "lut endpoint: got ({cr},{cg},{cb}) expect ({expect_r},0,0)"
    );
}
