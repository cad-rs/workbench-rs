//! Generic wgpu viewport render core for workbench-rs products.
//!
//! Orthographic-camera scene renderer: world-space triangles + line
//! segments → GPU pipelines (MVP projection, depth buffer, headlight
//! shading, MSAA 4x, screen-space line width) drawn into a host-provided
//! color target. Zero GUI dependencies: device, queue and target format
//! are handed in by the host ([`Renderer::new`]) — the crate never creates
//! a window and never touches a GUI type.
//!
//! Key design:
//! - **Two presentation paths**: [`Renderer::render`] draws straight into
//!   the host's color view (a swap-chain image on Windows, a texture GPUI
//!   samples on the web — the frame never leaves the GPU);
//!   [`Renderer::render_readback`] maps back a tightly-packed BGRA bitmap
//!   (headless pixel tests / other desktop shells falling back to
//!   `paint_image`).
//! - **Screen-space line width**: line segments are not 1px `LineList` —
//!   the vertex shader expands each segment (one instance = both ends)
//!   into a width quad: perpendicular offset in physical pixels (viewport
//!   size in the uniform, so thickness is resolution-independent) with
//!   depth interpolated per corner between the endpoints' NDC depths
//!   (ortho w=1 → linear, occlusion identical to the old line path).
//!   MSAA 4x smooths the quad edges.
//! - **Headless-testable**: tests build a local device and assert on
//!   readback pixels (tests/render.rs).
//!
//! Interactive camera semantics (orbit/pan/zoom + view presets,
//! [`CamOffset`]) live here too — the GUI shell only forwards raw
//! pointer/wheel input via [`cam_orbit`]/[`cam_pan`]/[`cam_zoom`]; see
//! `gpu_view::host` in workbench-ui for the generic shell element.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};

/// Render vertex: world position + normal + color (triangles and lines share the pipeline).
/// color.a is fixed at 1 in the opaque bucket's semantics; the transparent bucket
/// ([`RenderScene::tris_alpha`]) carries opacity.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct RenderVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
}

/// Line vertex (pos + color; the shader handles facing the camera). The vertex buffer is laid out
/// as **one instance = both ends of a segment**, back to back, and the vertex shader expands each
/// segment into a screen-space line-width quad (see [`SHADER_LINE`]). alpha is always 1
/// (transparent lines are outside the P0 semantics).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct LineVertexGpu {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

/// Camera: orthographic projection (yaw/pitch in degrees + the domain's bounding volume).
#[derive(Clone, Copy, Debug, Default)]
pub struct OrthoCamera {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    /// Domain center (world coordinates).
    pub center: [f32; 3],
    /// Domain radius (bounding sphere; ortho half-height = radius).
    pub radius: f32,
}

/// Frame parameters (uniform; members in the uniform address space must be 16-byte aligned).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct FrameUniform {
    mvp: [[f32; 4]; 4],
    light_dir: [f32; 3],
    _pad: u32,
    /// xy = viewport size in px; z = screen-space line width in px (for line expansion);
    /// w = padding.
    viewport: [f32; 4],
}

/// Render scene (plain data; the output of the product's geometry build, turned into GPU vertices).
/// `Default` is the empty scene (the placeholder left once the host element moves the product's
/// scene out).
#[derive(Default)]
pub struct RenderScene {
    /// Opaque bucket (depth write on; alpha is always 1).
    pub tris: Vec<RenderVertex>,
    /// Transparent bucket (round-205 P0: depth write **OFF**, read-only depth `Less`,
    /// `ALPHA_BLENDING`, draw order = array order; group-level far-to-near sorting is product-side
    /// domain knowledge, per-triangle sorting within the bucket is P1). Headlight shading still
    /// applies to rgb; alpha only carries opacity.
    pub tris_alpha: Vec<RenderVertex>,
    /// Transparent-bucket centroid sort switch (round-206 P1): when true, the `tris_alpha` vertices
    /// are reordered far-to-near by each triangle's centroid distance to the camera before drawing
    /// (stable: equal distances keep declaration order). The sort runs inside
    /// [`Renderer::render`]: the final camera (view preset + interactive camera offset) is known
    /// there, so the sort matches the final view direction. This is the layering where mechanical
    /// geometry belongs to the framework and domain grouping to the product (ask doc §6-5). For
    /// convex, non-intersecting triangles this is the correct blend order; intersecting/concave
    /// shapes are still approximated layer by layer (no per-pixel OIT is promised).
    /// false = P0 semantics (array order = blend order).
    pub sort_alpha: bool,
    /// Opaque lines (unchanged).
    pub lines: Vec<LineVertexGpu>,
    pub camera: OrthoCamera,
    pub bg: [f32; 4],
    /// Screen-space line width (px): the full width of the quads lines expand into; the physical
    /// thickness stays the same across viewport resolutions.
    pub line_width_px: f32,
    /// Volume render slice (round-211 P2 MVP, `docs/multiphysics-volume-render-ask.md` §3 data
    /// contract): passes "texture + bounding box + LUT", not a "field"; the framework knows nothing
    /// of fields. None = the existing semantics, zero regression. Draw order tris → lines →
    /// **volume** → tris_alpha (the volume is the farthest transparent layer, ask doc §5.2).
    pub volume: Option<VolumeSlice>,
}

/// Volume render data contract: a scalar volume (normalized to [0,1], row-major with x fastest,
/// cell-centered), a world AABB, a LUT (256×rgba8: rgb color scale / a opacity curve, both made on
/// the product side), the sample step count (the quality/cost knob) and a global opacity
/// multiplier. `generation` is the resampling generation: (dims, generation) is the 3D texture
/// upload cache key, and an unchanged generation is not re-uploaded (the product rebuilds the scene
/// every frame; MB-scale volume data avoids re-uploads through Arc sharing + the generation cache).
#[derive(Clone, Debug)]
pub struct VolumeSlice {
    pub data: std::sync::Arc<[f32]>,
    pub dims: [u32; 3],
    pub bounds: ([f32; 3], [f32; 3]),
    pub lut: Vec<[u8; 4]>,
    pub steps: u32,
    pub alpha_scale: f32,
    pub generation: u64,
}

impl RenderScene {
    /// Transparent-bucket centroid far-to-near sort (the pre-draw reorder while
    /// [`RenderScene::sort_alpha`] is on): triangles in descending order of centroid-to-eye
    /// distance (farthest drawn first), keys precomputed in O(n) + a stable sort, so equal
    /// distances keep declaration order (what remains of the group-level emission-order
    /// semantics). Returns None when the switch is off or there are fewer than two triangles
    /// (nothing to order), and the caller falls back to array order.
    pub fn alpha_sorted(&self) -> Option<Vec<RenderVertex>> {
        if !self.sort_alpha || self.tris_alpha.len() < 6 {
            return None;
        }
        let eye = eye_pos(&self.camera);
        let n_tris = self.tris_alpha.len() / 3;
        let centroid_d2 = |t: usize| -> f32 {
            let v = &self.tris_alpha[t * 3..t * 3 + 3];
            let d = [
                (v[0].pos[0] + v[1].pos[0] + v[2].pos[0]) / 3.0 - eye[0],
                (v[0].pos[1] + v[1].pos[1] + v[2].pos[1]) / 3.0 - eye[1],
                (v[0].pos[2] + v[1].pos[2] + v[2].pos[2]) / 3.0 - eye[2],
            ];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
        };
        let mut order: Vec<(u32, f32)> = (0..n_tris as u32)
            .map(|t| {
                let t = t as usize;
                (t as u32, centroid_d2(t))
            })
            .collect();
        order.sort_by(|a, b| b.1.total_cmp(&a.1));
        Some(
            order
                .iter()
                .flat_map(|&(t, _)| self.tris_alpha[t as usize * 3..t as usize * 3 + 3].iter())
                .copied()
                .collect(),
        )
    }
}

// ---- Interactive camera (viewport semantics belong to the render core; the GUI shell only
// forwards raw input) ----
//
// Orbit sensitivity, the pan's world conversion and the zoom clamp all live in this module; the
// GUI shell only passes raw mouse offsets / wheel parameters to
// [`cam_orbit`]/[`cam_pan`]/[`cam_zoom`].

/// View presets (in panel button order): (label, yaw°, pitch°). Top-down doesn't degenerate (the
/// projection's right vector depends only on yaw).
pub const VIEW_ANGLES: [(&str, f64, f64); 4] = [
    ("Iso", 35.0, 30.0),
    ("Front", 0.0, 0.0),
    ("Right", 90.0, 0.0),
    ("Top", 0.0, 90.0),
];

/// Current view preset index (a session-level view preference; switched by the panel buttons).
pub static VIEW_ANGLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Interactive camera offset (session-level, not persisted): orbit drag (yaw/pitch layered on the
/// view preset) + wheel zoom (radius multiplier) + Shift-drag pan (a screen-space offset; the
/// world conversion happens in [`CamOffset::apply_to`] against the current field of view). The
/// view preset buttons reset it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamOffset {
    pub yaw: f64,
    pub pitch: f64,
    pub zoom: f64,
    /// Accumulated screen-space pan (px; +x right, +y down).
    pub pan: [f64; 2],
}

impl Default for CamOffset {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
            pan: [0.0, 0.0],
        }
    }
}

impl CamOffset {
    /// Wheel zoom (multiplier clamped to 0.15..6, about 1.6 orders of magnitude of field of view).
    pub fn zoom_by(&mut self, factor: f64) {
        self.zoom = (self.zoom * factor).clamp(0.15, 6.0);
    }

    /// Orbit drag (degrees/pixel × offset).
    pub fn orbit_by(&mut self, dx: f64, dy: f64) {
        self.yaw += dx * 0.4;
        self.pitch += dy * 0.4;
    }

    /// Pan drag (accumulated screen pixels; clamped to ± a few viewports so the view isn't lost).
    pub fn pan_by(&mut self, dx: f64, dy: f64) {
        self.pan[0] = (self.pan[0] + dx).clamp(-4000.0, 4000.0);
        self.pan[1] = (self.pan[1] + dy).clamp(-4000.0, 4000.0);
    }

    /// Combines preset + offset into the final ortho camera (rewrites yaw/pitch/radius/center in
    /// place). Pan conversion: screen px → NDC (2×half-height/viewport height, zoom included) →
    /// world (right/up basis).
    pub fn apply_to(&self, preset_yaw: f64, preset_pitch: f64, cam: &mut OrthoCamera, view_h_px: f64) {
        let radius = cam.radius as f64; // base radius (before zoom)
        cam.yaw_deg = (preset_yaw + self.yaw) as f32;
        cam.pitch_deg = (preset_pitch + self.pitch) as f32;
        // World units/px: ortho half-height = radius×1.2×zoom → NDC height 2.
        let world_per_px = 2.0 * radius * 1.2 * self.zoom / view_h_px.max(1.0);
        let (cy, sy, cp, sp) = (
            (cam.yaw_deg as f64).to_radians().cos(),
            (cam.yaw_deg as f64).to_radians().sin(),
            (cam.pitch_deg as f64).to_radians().cos(),
            (cam.pitch_deg as f64).to_radians().sin(),
        );
        let r = [-sy, cy, 0.0];
        let u = [-sp * cy, -sp * sy, cp];
        // Drag +x = camera moves left (scene moves right) → center -= r·dx;
        // +y down = center += u·dy.
        for k in 0..3 {
            cam.center[k] += ((-r[k] * self.pan[0] + u[k] * self.pan[1]) * world_per_px) as f32;
        }
        cam.radius = (radius * self.zoom) as f32;
    }
}

thread_local! {
    static CAM: std::cell::RefCell<CamOffset> = const {
        std::cell::RefCell::new(CamOffset {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
            pan: [0.0, 0.0],
        })
    };
}

/// Raw shell input → orbit drag (offset in px; the sensitivity semantics live in this module).
pub fn cam_orbit(dx: f64, dy: f64) {
    CAM.with(|c| c.borrow_mut().orbit_by(dx, dy));
}

/// Raw shell input → Shift pan drag (offset in px).
pub fn cam_pan(dx: f64, dy: f64) {
    CAM.with(|c| c.borrow_mut().pan_by(dx, dy));
}

/// Raw shell input → wheel zoom (multiplier).
pub fn cam_zoom(factor: f64) {
    CAM.with(|c| c.borrow_mut().zoom_by(factor));
}

/// Combines preset + offset into the final camera (switching view presets calls
/// [`reset_cam_offset`] first).
pub fn cam_apply(preset_yaw: f64, preset_pitch: f64, cam: &mut OrthoCamera, view_h_px: f64) {
    CAM.with(|c| c.borrow().apply_to(preset_yaw, preset_pitch, cam, view_h_px));
}

/// Resets the camera offset (called when the view preset switches).
pub fn reset_cam_offset() {
    CAM.with(|c| *c.borrow_mut() = CamOffset::default());
}

/// The current camera offset (test accessor).
pub fn cam_offset_for_test() -> CamOffset {
    CAM.with(|c| *c.borrow())
}

/// Volume render frame parameters (uniform; 16-byte aligned members).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct VolumeUniform {
    mvp: [[f32; 4]; 4],
    /// World AABB (w = 0).
    aabb_lo: [f32; 4],
    aabb_hi: [f32; 4],
    /// Camera eye point (for computing view depth = d·(eye - p)).
    eye: [f32; 4],
    /// View ray direction (into the scene; constant for ortho = -headlight direction).
    ray_dir: [f32; 4],
    /// x = near, y = f - near, z = steps, w = alpha_scale.
    params: [f32; 4],
}

/// f32 → f16 bit pattern (IEEE 754 half): ±Inf/NaN keep their kind, overflow → Inf, underflow →
/// signed zero (normalized [0,1] volume data never reaches the subnormal range), mantissa
/// round-to-nearest (ties simplified to rounding up on `> half a step`, no difference for volume
/// sampling). Used for the R16Float upload without adding a dependency (ask doc §10-5, the
/// preference was settled).
fn f16_bits(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32;
    let mant = b & 0x007F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | u16::from(mant != 0);
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7C00;
    }
    if e <= 0 {
        return sign;
    }
    let h = sign | ((e as u16) << 10) | ((mant >> 13) as u16);
    if mant & 0x1000 != 0 && mant & 0x0FFF != 0 {
        h + 1
    } else {
        h
    }
}

/// Column-major 4×4.
type Mat4 = [[f32; 4]; 4];

fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [[0.0f32; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            o[c][r] = a[0][r] * b[c][0] + a[1][r] * b[c][1] + a[2][r] * b[c][2] + a[3][r] * b[c][3];
        }
    }
    o
}

/// Camera eye point (the ortho camera sits at center + view direction × 3r; shared by `ortho_mvp`
/// and the transparent-bucket centroid sort, so sort distances and the projection's view
/// direction come from the same source).
fn eye_pos(cam: &OrthoCamera) -> [f32; 3] {
    let (yaw, pitch) = (cam.yaw_deg.to_radians(), cam.pitch_deg.to_radians());
    let d = [yaw.cos() * pitch.cos(), yaw.sin() * pitch.cos(), pitch.sin()];
    [
        cam.center[0] + d[0] * cam.radius * 3.0,
        cam.center[1] + d[1] * cam.radius * 3.0,
        cam.center[2] + d[2] * cam.radius * 3.0,
    ]
}

/// Depth range (near, f - near): the parameters of the linear ortho NDC z mapping, shared by
/// `ortho_mvp` and the volume pass's wall-hit view-depth resolve (round-211).
fn depth_range(cam: &OrthoCamera) -> (f32, f32) {
    (0.01 * cam.radius.max(1e-6), cam.radius * 8.0)
}

/// Ortho MVP (view-lookat + ortho; the camera at (0,0,3r) looks at the center, rotated by
/// yaw/pitch first).
fn ortho_mvp(cam: &OrthoCamera, aspect: f32) -> Mat4 {
    let (yaw, pitch) = (cam.yaw_deg.to_radians(), cam.pitch_deg.to_radians());
    let (cy, sy, cp, sp) = (yaw.cos(), yaw.sin(), pitch.cos(), pitch.sin());
    // Camera basis (right-handed, world up = +z): d points at the camera, r right, u up.
    let d = [cp * cy, cp * sy, sp];
    let r = [-sy, cy, 0.0];
    let u = [-sp * cy, -sp * sy, cp];
    let eye = eye_pos(cam);
    // view = [r; u; -d] + translation (built row-major, then transposed to column-major).
    let view_rows: [[f32; 4]; 4] = [
        [r[0], r[1], r[2], -(r[0] * eye[0] + r[1] * eye[1] + r[2] * eye[2])],
        [u[0], u[1], u[2], -(u[0] * eye[0] + u[1] * eye[1] + u[2] * eye[2])],
        [-d[0], -d[1], -d[2], d[0] * eye[0] + d[1] * eye[1] + d[2] * eye[2]],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut view = [[0.0f32; 4]; 4];
    for c in 0..4 {
        for rr in 0..4 {
            view[c][rr] = view_rows[rr][c];
        }
    }
    // Orthographic projection (half-height = 1.2×radius for a margin; near/far cover [0.01r, 8r]
    // in front of the camera).
    // **WebGPU NDC depth is [0,1]** (not OpenGL's [-1,1]): z_ndc = (z-n)/(f-n).
    let hh = cam.radius * 1.2;
    let hw = hh * aspect.max(1e-3);
    let (n, f) = depth_range(cam);
    let mut proj = [[0.0f32; 4]; 4];
    proj[0][0] = 1.0 / hw;
    proj[1][1] = 1.0 / hh;
    proj[2][2] = 1.0 / (f - n);
    proj[2][3] = -n / (f - n);
    proj[3][3] = 1.0;
    mat_mul(&proj, &view)
}

const SHADER: &str = r#"
struct FrameUniform {
    mvp: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad: u32,
    viewport: vec4<f32>, // unused by the triangle pipeline (same uniform layout as the line one).
};
@group(0) @binding(0) var<uniform> frame: FrameUniform;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>,
           @location(1) normal: vec3<f32>,
           @location(2) color: vec4<f32>) -> VSOut {
    var out: VSOut;
    out.pos = frame.mvp * vec4<f32>(pos, 1.0);
    out.world_normal = normal;
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    // Headlight: 0.45 ambient + 0.55 |n·l| (shading multiplies into rgb, alpha only carries
    // opacity: the transparent-bucket pipeline uses ALPHA_BLENDING, and under the opaque bucket's
    // REPLACE a=1 behaves as before).
    let l = normalize(frame.light_dir);
    let nd = abs(dot(normalize(in.world_normal), l));
    let shade = 0.45 + 0.55 * nd;
    return vec4<f32>(in.color.rgb * shade, in.color.a);
}
"#;

const SHADER_LINE: &str = r#"
struct FrameUniform {
    mvp: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad: u32,
    viewport: vec4<f32>, // xy = viewport px; z = screen-space line width px.
};
@group(0) @binding(0) var<uniform> frame: FrameUniform;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// Six-vertex corner table (two triangles making up a segment's quad): x = parameter t along the
// segment, y = perpendicular sign.
fn corner(vi: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    return c[vi];
}

// Screen-space line width: one instance reads both ends of a segment, takes each through the MVP
// into clip space, then expands by half the width along the perpendicular in screen px (the
// NDC→px→NDC conversion includes the viewport size, so the physical thickness doesn't change with
// resolution); depth is interpolated linearly between the two ends' NDC depths by the corner
// parameter. Ortho projection keeps w=1, so occlusion matches the 1px LineList.
@vertex
fn vs_main(@location(0) pos_a: vec3<f32>,
           @location(1) color_a: vec4<f32>,
           @location(2) pos_b: vec3<f32>,
           @location(3) color_b: vec4<f32>,
           @builtin(vertex_index) vi: u32) -> VSOut {
    let k = corner(vi);
    let ca = frame.mvp * vec4<f32>(pos_a, 1.0);
    let cb = frame.mvp * vec4<f32>(pos_b, 1.0);
    let na = ca.xy / ca.w;
    let nb = cb.xy / cb.w;
    // Line direction NDC→px (px = ndc × 0.5×viewport); take the screen-space perpendicular unit
    // vector.
    let to_px = 0.5 * frame.viewport.xy;
    let d = (nb - na) * to_px;
    let len = length(d);
    let perp = select(vec2<f32>(1.0, 0.0), vec2<f32>(-d.y, d.x) / len, len > 1e-6);
    // Half width px → NDC (per component ÷ half viewport px); corners expand along the
    // perpendicular.
    let off = perp * (0.5 * frame.viewport.z / to_px) * k.y;
    var out: VSOut;
    out.pos = vec4<f32>(
        mix(na, nb, k.x) + off,
        mix(ca.z / ca.w, cb.z / cb.w, k.x),
        1.0);
    out.color = mix(color_a, color_b, k.x);
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

const SHADER_VOLUME: &str = r#"
struct VolumeUniform {
    mvp: mat4x4<f32>,
    aabb_lo: vec4<f32>,
    aabb_hi: vec4<f32>,
    eye: vec4<f32>,
    ray_dir: vec4<f32>,  // into the scene (constant for ortho).
    params: vec4<f32>,   // x=near y=f-near z=steps w=alpha_scale
};
@group(0) @binding(0) var<uniform> u: VolumeUniform;
@group(0) @binding(1) var vol_tex: texture_3d<f32>;
@group(0) @binding(2) var vol_samp: sampler;
@group(0) @binding(3) var wall_tex: texture_depth_2d;
@group(0) @binding(4) var lut_tex: texture_1d<f32>;

// Unit cube, 12 triangles (36 vertices, outward CCW: cross(v1-v0, v2-v0) = outward normal; after
// back-face culling only the faces toward the camera remain, i.e. the ray entry faces).
fn cube(vi: u32) -> vec3<f32> {
    var c = array<vec3<f32>, 36>(
        vec3<f32>(0., 0., 1.), vec3<f32>(1., 0., 1.), vec3<f32>(1., 1., 1.),
        vec3<f32>(0., 0., 1.), vec3<f32>(1., 1., 1.), vec3<f32>(0., 1., 1.),
        vec3<f32>(0., 0., 0.), vec3<f32>(0., 1., 0.), vec3<f32>(1., 1., 0.),
        vec3<f32>(0., 0., 0.), vec3<f32>(1., 1., 0.), vec3<f32>(1., 0., 0.),
        vec3<f32>(1., 0., 0.), vec3<f32>(1., 1., 1.), vec3<f32>(1., 0., 1.),
        vec3<f32>(1., 0., 0.), vec3<f32>(1., 1., 0.), vec3<f32>(1., 1., 1.),
        vec3<f32>(0., 0., 0.), vec3<f32>(0., 0., 1.), vec3<f32>(0., 1., 1.),
        vec3<f32>(0., 0., 0.), vec3<f32>(0., 1., 1.), vec3<f32>(0., 1., 0.),
        vec3<f32>(0., 1., 0.), vec3<f32>(0., 1., 1.), vec3<f32>(1., 1., 1.),
        vec3<f32>(0., 1., 0.), vec3<f32>(1., 1., 1.), vec3<f32>(1., 1., 0.),
        vec3<f32>(0., 0., 0.), vec3<f32>(1., 0., 0.), vec3<f32>(1., 0., 1.),
        vec3<f32>(0., 0., 0.), vec3<f32>(1., 0., 1.), vec3<f32>(0., 0., 1.));
    return c[vi];
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let c = cube(vi);
    var out: VsOut;
    out.pos = u.mvp * vec4<f32>(mix(u.aabb_lo.xyz, u.aabb_hi.xyz, c), 1.0);
    out.world = mix(u.aabb_lo.xyz, u.aabb_hi.xyz, c);
    return out;
}

// Ray-march from the bounding box's front faces: the ortho ray direction is constant across the
// screen, the start = the front fragment's world position; the exit = the AABB slab solve; the
// wall hit = view depth resolved from the 1x depth pre-pass texel (ortho w=1 → depth is a linear
// mapping of NDC z). Equidistant samples → LUT → front-to-back accumulation, with early ray
// termination at acc_a > 0.99; the output is premultiplied (pipeline ONE +
// ONE_MINUS_SRC_ALPHA).
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let ro = in.world;
    let rd = normalize(u.ray_dir.xyz);
    var t_far = 1e30;
    for (var k = 0; k < 3; k++) {
        if (abs(rd[k]) < 1e-12) { continue; } // parallel axis: ro in the slab, no constraint
        let t1 = (u.aabb_lo[k] - ro[k]) / rd[k];
        let t2 = (u.aabb_hi[k] - ro[k]) / rd[k];
        t_far = min(t_far, max(t1, t2));
    }
    let depth = textureLoad(wall_tex, vec2<i32>(in.pos.xy), 0);
    let view_z_wall = depth * u.params.y + u.params.x;
    let view_z_ro = dot(u.eye.xyz - ro, -rd);
    let t_wall = max(view_z_wall - view_z_ro, 0.0);
    let t_end = max(min(t_far, t_wall), 0.0);
    let steps = u32(u.params.z + 0.5);
    let dt = t_end / f32(steps);
    let size = u.aabb_hi.xyz - u.aabb_lo.xyz;
    var acc = vec3<f32>(0.0);
    var acc_a = 0.0;
    for (var s = u32(0); s < steps; s++) {
        if (acc_a > 0.99) { break; }
        let p = ro + rd * (f32(s) * dt + 0.5 * dt);
        let uvw = (p - u.aabb_lo.xyz) / size;
        if (any(uvw < vec3<f32>(0.0)) || any(uvw > vec3<f32>(1.0))) { continue; }
        let v = textureSampleLevel(vol_tex, vol_samp, uvw, 0.0).r;
        let rgba = textureSampleLevel(lut_tex, vol_samp, v, 0.0);
        let a = rgba.a * u.params.w;
        acc += (1.0 - acc_a) * a * rgba.rgb;
        acc_a += (1.0 - acc_a) * a;
    }
    return vec4<f32>(acc, acc_a);
}
"#;

/// Pure wgpu render core: depends only on wgpu types. The host passes in the device/queue/target
/// format (the GUI shell passes the shared device and goes through surface composition; headless
/// tests build a local device); this module never creates a window or touches any GUI type.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,
    tri_pipeline: wgpu::RenderPipeline,
    tri_alpha_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    /// Volume ray-march pipeline (the fourth pipeline in the main MSAA pass, round-211 option I).
    volume_pipeline: wgpu::RenderPipeline,
    /// 1x depth pre-pass pipeline (no fragment, sample_count 1; for the wall-hit cutoff).
    depth_pre_pipeline: wgpu::RenderPipeline,
    volume_bgl: wgpu::BindGroupLayout,
    volume_uniform: wgpu::Buffer,
    volume_bind: Option<wgpu::BindGroup>,
    /// The 3D data texture + its (dims, generation) cache key; an unchanged generation isn't
    /// re-uploaded.
    volume_tex: Option<(wgpu::Texture, [u32; 3], u64)>,
    /// LUT 1D texture (256×rgba8; about 1KB, re-uploaded every frame, so no cache key).
    lut_tex: Option<wgpu::Texture>,
    /// 1x depth pre-pass target (Depth24Plus, RENDER_ATTACHMENT |
    /// TEXTURE_BINDING; rebuilt on resize).
    wall: Option<(wgpu::TextureView, u32, u32)>,
    uniform_buf: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    tri_buf: wgpu::Buffer,
    tri_alpha_buf: wgpu::Buffer,
    line_buf: wgpu::Buffer,
    n_tris: u32,
    n_tris_alpha: u32,
    n_lines: u32,
    /// Depth target (shared by both presentation paths; rebuilt on resize; same MSAA count as
    /// color).
    depth: Option<(wgpu::TextureView, u32, u32)>,
    /// MSAA color target (resolves into the color view the host passes in; rebuilt on resize).
    msaa: Option<(wgpu::TextureView, u32, u32)>,
    /// Readback target (for `render_readback` only: color texture + readback buffer).
    readback: Option<ReadbackTarget>,
}

/// An offscreen color target that can be read back (color texture + aligned readback buffer):
/// shared by `render_readback` and the shell's readback presentation path.
pub struct ReadbackTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buf: wgpu::Buffer,
    width: u32,
    height: u32,
    bytes_per_row: u32,
}

impl ReadbackTarget {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wb-render-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let bytes_per_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wb-render-readback"),
            size: bytes_per_row as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view, buf, width, height, bytes_per_row }
    }

    /// Size (device pixels).
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The render target view.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Copies out and blocks until done: tightly packed BGRA rows (`width*height*4`, row padding
    /// removed).
    pub fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Vec<u8>> {
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("wb-render-readback-enc"),
        });
        enc.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        queue.submit(Some(enc.finish()));
        let (sx, rx) = std::sync::mpsc::channel();
        let slice = self.buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = sx.send(res);
        });
        device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
            .ok()?;
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let row = self.width as usize * 4;
        let mut out = Vec::with_capacity(row * self.height as usize);
        for chunk in data.chunks(self.bytes_per_row as usize).take(self.height as usize) {
            out.extend_from_slice(&chunk[..row]);
        }
        drop(data);
        self.buf.unmap();
        Some(out)
    }
}

impl Renderer {
    /// MSAA sample count (4x: the WebGPU spec guarantees support for renderable formats).
    const MSAA_SAMPLES: u32 = 4;

    /// Construction on the shared host device (no failure path: the host guarantees a usable
    /// device/queue).
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, target_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wb-render-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let module_line = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wb-render-shader-line"),
            source: wgpu::ShaderSource::Wgsl(SHADER_LINE.into()),
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wb-render-uniform"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wb-render-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<FrameUniform>() as u64),
                },
                count: None,
            }],
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wb-render-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let make_pipeline = |module: &wgpu::ShaderModule,
                             line: bool,
                             alpha: bool|
         -> wgpu::RenderPipeline {
            // Line pipeline: one instance = both ends of a segment (2×LineVertexGpu back to back),
            // and the vertex shader expands a 6-vertex quad; the triangle pipeline goes per
            // vertex. Both are TriangleList + depth + MSAA. Transparent triangle pipeline
            // (alpha=true): ALPHA_BLENDING + depth write OFF (read-only depth Less); draw order is
            // blend order, see the RenderScene::tris_alpha docs for the sorting semantics.
            let vb_layout = [if line {
                wgpu::VertexBufferLayout {
                    array_stride: (2 * size_of::<LineVertexGpu>()) as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 28,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 40,
                            shader_location: 3,
                        },
                    ],
                }
            } else {
                wgpu::VertexBufferLayout {
                    array_stride: size_of::<RenderVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 24,
                            shader_location: 2,
                        },
                    ],
                }
            }];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if line {
                    "wb-render-pipeline-line"
                } else if alpha {
                    "wb-render-pipeline-tri-alpha"
                } else {
                    "wb-render-pipeline-tri"
                }),
                layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("wb-render-pl"),
                    bind_group_layouts: &[Some(&bgl)],
                    immediate_size: 0,
                })),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &vb_layout.map(Some),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(if alpha {
                            wgpu::BlendState::ALPHA_BLENDING
                        } else {
                            wgpu::BlendState::REPLACE
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth24Plus,
                    depth_write_enabled: Some(!alpha),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: Self::MSAA_SAMPLES,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let tri_pipeline = make_pipeline(&module, false, false);
        let line_pipeline = make_pipeline(&module_line, true, false);
        let tri_alpha_pipeline = make_pipeline(&module, false, true);

        // ---- Volume rendering (round-211 P2 option I: a fourth pipeline in the main MSAA pass +
        // a 1x depth pre-pass; the existing three pipelines are untouched, byte for byte) ----
        let module_volume = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wb-render-shader-volume"),
            source: wgpu::ShaderSource::Wgsl(SHADER_VOLUME.into()),
        });
        let volume_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wb-render-bgl-volume"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(size_of::<VolumeUniform>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D1,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let volume_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("wb-render-pl-volume"),
                bind_group_layouts: &[Some(&volume_bgl)],
                immediate_size: 0,
            });
        let volume_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wb-render-pipeline-volume"),
            layout: Some(&volume_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module_volume,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module_volume,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                // premultiplied output: ONE + ONE_MINUS_SRC_ALPHA (coexisting, each in its own
                // pipeline, with the transparent bucket's non-premultiplied ALPHA_BLENDING).
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Outward-CCW cube with back-face culling → only the faces toward the camera
                // remain (the ray entry faces).
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: Self::MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        // 1x depth pre-pass: reuses the triangle pipeline's vertex stage (SHADER vs_main), no
        // fragment, sample_count 1; it only writes a dedicated depth texture for the ray-march's
        // wall hit.
        let depth_pre_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wb-render-pipeline-depth-pre"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("wb-render-pl-depth-pre"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            })),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<RenderVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 24,
                            shader_location: 2,
                        },
                    ],
                })],
            },
            fragment: None,
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let volume_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wb-render-volume-uniform"),
            size: size_of::<VolumeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tri_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wb-render-tri"),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tri_alpha_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wb-render-tri-alpha"),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let line_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wb-render-line"),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            device,
            queue,
            target_format,
            tri_pipeline,
            tri_alpha_pipeline,
            line_pipeline,
            volume_pipeline,
            depth_pre_pipeline,
            volume_bgl,
            volume_uniform,
            volume_bind: None,
            volume_tex: None,
            lut_tex: None,
            wall: None,
            uniform_buf,
            uniform_bind,
            tri_buf,
            tri_alpha_buf,
            line_buf,
            n_tris: 0,
            n_tris_alpha: 0,
            n_lines: 0,
            depth: None,
            msaa: None,
            readback: None,
        }
    }

    fn ensure_depth(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        let (w, h) = (width.max(1), height.max(1));
        if let Some((view, dw, dh)) = &self.depth {
            if *dw == w && *dh == h {
                return view.clone();
            }
        }
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wb-render-depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: Self::MSAA_SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = depth.create_view(&Default::default());
        self.depth = Some((view.clone(), w, h));
        view
    }

    /// Ensures the MSAA color target (it resolves into the host view; reused while the size is
    /// unchanged).
    fn ensure_msaa(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        let (w, h) = (width.max(1), height.max(1));
        if let Some((view, mw, mh)) = &self.msaa {
            if *mw == w && *mh == h {
                return view.clone();
            }
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wb-render-msaa"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: Self::MSAA_SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.msaa = Some((view.clone(), w, h));
        view
    }

    /// Ensures the 1x depth pre-pass target (wall-hit cutoff; reused while the size is unchanged).
    fn ensure_wall(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        let (w, h) = (width.max(1), height.max(1));
        if let Some((view, dw, dh)) = &self.wall {
            if *dw == w && *dh == h {
                return view.clone();
            }
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wb-render-depth-wall"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.wall = Some((view.clone(), w, h));
        view
    }

    /// Ensures the volume resources: the 3D data texture is cached by (dims, generation) (an
    /// unchanged generation isn't re-uploaded), the 1D LUT is re-uploaded every frame (~1KB), and
    /// the bind group is rebuilt along with the textures. `wall_view` = the 1x depth pre-pass
    /// target's view (the wall-hit texels; made ready before this call).
    fn ensure_volume(&mut self, v: &VolumeSlice, wall_view: &wgpu::TextureView) -> wgpu::BindGroup {
        let rebuilt = match (&self.volume_tex, &self.lut_tex) {
            (Some((_, d, g)), Some(_)) if *d == v.dims && *g == v.generation => None,
            _ => Some(()),
        };
        if let Some(()) = rebuilt {
            // R16Float 3D (row-major, x fastest, cell-centered); the upload pads each row to a
            // 256 alignment (write_texture's bpr semantics check the source length across
            // rows/layers).
            let bpr = (v.dims[0] * 2).next_multiple_of(256) as usize;
            let row = v.dims[0] as usize;
            let mut packed: Vec<u16> = Vec::with_capacity(bpr / 2 * v.dims[1] as usize * v.dims[2] as usize);
            for z in 0..v.dims[2] as usize {
                for y in 0..v.dims[1] as usize {
                    let base = (z * v.dims[1] as usize + y) * row;
                    packed.extend(v.data[base..base + row].iter().map(|&x| f16_bits(x)));
                    packed.resize(packed.len() + (bpr - row * 2) / 2, 0);
                }
            }
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("wb-render-volume"),
                size: wgpu::Extent3d {
                    width: v.dims[0],
                    height: v.dims[1],
                    depth_or_array_layers: v.dims[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::R16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&packed),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr as u32),
                    rows_per_image: Some(v.dims[1]),
                },
                wgpu::Extent3d {
                    width: v.dims[0],
                    height: v.dims[1],
                    depth_or_array_layers: v.dims[2],
                },
            );
            let lut = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("wb-render-volume-lut"),
                size: wgpu::Extent3d {
                    width: v.lut.len().max(1) as u32,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D1,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.volume_tex = Some((texture, v.dims, v.generation));
            self.lut_tex = Some(lut);
            self.volume_bind = None; // texture rebuilt → the bind group is rebuilt with it.
        }
        let (_, lut) = match (&self.volume_tex, &self.lut_tex) {
            (Some((t, _, _)), Some(l)) => (t, l),
            _ => unreachable!("volume textures just ensured"),
        };
        // Re-upload the LUT (~1KB every frame; write_texture's semantics are enough).
        let lut_bytes: Vec<u8> = v.lut.iter().flatten().copied().collect();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: lut,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &lut_bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: None, rows_per_image: None },
            wgpu::Extent3d { width: v.lut.len().max(1) as u32, height: 1, depth_or_array_layers: 1 },
        );
        if self.volume_bind.is_none() {
            let (tex, _, _) = self.volume_tex.as_ref().unwrap();
            let volume_view =
                tex.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("wb-render-volume-view"),
                    dimension: Some(wgpu::TextureViewDimension::D3),
                    ..Default::default()
                });
            let lut_view = lut.create_view(&Default::default());
            let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("wb-render-volume-sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            self.volume_bind = Some(self.device.create_bind_group(
                &wgpu::BindGroupDescriptor {
                    label: Some("wb-render-bg-volume"),
                    layout: &self.volume_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.volume_uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&volume_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(wall_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&lut_view),
                        },
                    ],
                },
            ));
        }
        self.volume_bind.clone().unwrap()
    }
    /// Ensures the readback target (reused while the size is unchanged).
    fn ensure_readback(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        let size = (width.max(1), height.max(1));
        if self.readback.as_ref().is_none_or(|t| t.size() != size) {
            self.readback =
                Some(ReadbackTarget::new(&self.device, self.target_format, size.0, size.1));
        }
        self.readback.as_ref().expect("allocated above").view().clone()
    }

    /// Renders a frame into the host's color target: updates the mvp/headlight uniform, uploads the
    /// vertices and encodes the draws (the texture stays on the GPU throughout and the shell
    /// presents it per platform; the viewport renders on demand, not on a per-frame hot path).
    pub fn render(
        &mut self,
        scene: &RenderScene,
        width: u32,
        height: u32,
        color_view: &wgpu::TextureView,
    ) {
        let width = width.max(1);
        let height = height.max(1);
        let aspect = width as f32 / height as f32;
        // The direction the camera looks into the scene = the light direction (headlight).
        let (yaw, pitch) =
            (scene.camera.yaw_deg.to_radians(), scene.camera.pitch_deg.to_radians());
        let (cy, sy, cp, sp) = (yaw.cos(), yaw.sin(), pitch.cos(), pitch.sin());
        let light = [cp * cy, cp * sy, sp];
        let uni = FrameUniform {
            mvp: ortho_mvp(&scene.camera, aspect),
            light_dir: light,
            _pad: 0,
            viewport: [width as f32, height as f32, scene.line_width_px, 0.0],
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uni));
        // Vertex upload (growing the buffers when needed).
        let need_tri = (scene.tris.len() * size_of::<RenderVertex>()) as u64;
        if need_tri > self.tri_buf.size() {
            self.tri_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wb-render-tri"),
                size: need_tri.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !scene.tris.is_empty() {
            self.queue
                .write_buffer(&self.tri_buf, 0, bytemuck::cast_slice(&scene.tris));
        }
        // Transparent-bucket upload: `sort_alpha` on = centroid far-to-near reorder before drawing
        // (the final camera is known now: the interactive camera offset is already layered on, so
        // the sort matches the final view direction); off = array order (P0 semantics).
        let alpha_sorted = scene.alpha_sorted();
        let alpha_src: &[RenderVertex] = alpha_sorted.as_deref().unwrap_or(&scene.tris_alpha);
        let need_tri_alpha = size_of_val(alpha_src) as u64;
        if need_tri_alpha > self.tri_alpha_buf.size() {
            self.tri_alpha_buf = self
                .device
                .create_buffer(&wgpu::BufferDescriptor {
                    label: Some("wb-render-tri-alpha"),
                    size: need_tri_alpha.next_power_of_two(),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
        }
        if !alpha_src.is_empty() {
            self.queue
                .write_buffer(&self.tri_alpha_buf, 0, bytemuck::cast_slice(alpha_src));
        }
        let need_line = (scene.lines.len() * size_of::<LineVertexGpu>()) as u64;
        if need_line > self.line_buf.size() {
            self.line_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wb-render-line"),
                size: need_line.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !scene.lines.is_empty() {
            self.queue
                .write_buffer(&self.line_buf, 0, bytemuck::cast_slice(&scene.lines));
        }
        self.n_tris = scene.tris.len() as u32;
        self.n_tris_alpha = scene.tris_alpha.len() as u32;
        self.n_lines = scene.lines.len() as u32;
        // Volume rendering (round-211): uniform upload + texture/bind preparation (the depth
        // pre-pass target comes first, since the bind group references its view). In draw order
        // the volume comes before tris_alpha (the volume is the farthest transparent layer, ask
        // doc §5.2).
        let volume_draw = match &scene.volume {
            Some(v) => {
                let (yaw_v, pitch_v) =
                    (scene.camera.yaw_deg.to_radians(), scene.camera.pitch_deg.to_radians());
                let d = [yaw_v.cos() * pitch_v.cos(), yaw_v.sin() * pitch_v.cos(), pitch_v.sin()];
                let eye = eye_pos(&scene.camera);
                let (n, f) = depth_range(&scene.camera);
                let uni = VolumeUniform {
                    mvp: ortho_mvp(&scene.camera, aspect),
                    aabb_lo: [v.bounds.0[0], v.bounds.0[1], v.bounds.0[2], 0.0],
                    aabb_hi: [v.bounds.1[0], v.bounds.1[1], v.bounds.1[2], 0.0],
                    eye: [eye[0], eye[1], eye[2], 0.0],
                    ray_dir: [-d[0], -d[1], -d[2], 0.0],
                    params: [n, f - n, v.steps as f32, v.alpha_scale],
                };
                self.queue
                    .write_buffer(&self.volume_uniform, 0, bytemuck::bytes_of(&uni));
                let wall = self.ensure_wall(width, height);
                let bind = self.ensure_volume(v, &wall);
                Some((wall, bind))
            }
            None => None,
        };

        let depth_view = self.ensure_depth(width, height);
        let msaa_view = self.ensure_msaa(width, height);
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wb-render-enc"),
            });
        // 1x depth pre-pass (wall-hit cutoff, ask doc §5.1): the opaque triangles are redrawn with
        // the fragment-less pipeline into the dedicated 1x depth (microseconds for tens of
        // thousands of triangles); the ray-march reads one texel per pixel to resolve t_wall.
        if let Some((wall, _)) = &volume_draw {
            // The pass always runs (Clear 1.0 = far plane): with no opaque triangles the wall stays
            // at far depth and the ray-march's t_wall degenerates to t_exit.
            let mut pre = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wb-render-pass-depth-pre"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: wall,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.n_tris > 0 {
                pre.set_pipeline(&self.depth_pre_pipeline);
                pre.set_bind_group(0, &self.uniform_bind, &[]);
                pre.set_vertex_buffer(0, self.tri_buf.slice(..));
                pre.draw(0..self.n_tris, 0..1);
            }
        }
        {
            let bg = wgpu::LoadOp::Clear(wgpu::Color {
                r: scene.bg[0] as f64,
                g: scene.bg[1] as f64,
                b: scene.bg[2] as f64,
                a: scene.bg[3] as f64,
            });
            // Render into the MSAA attachment and resolve into the host's color view (the
            // swap-chain image / dynamic texture / readback texture all have RENDER_ATTACHMENT
            // usage).
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wb-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa_view,
                    resolve_target: Some(color_view),
                    ops: wgpu::Operations { load: bg, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.n_tris > 0 {
                pass.set_pipeline(&self.tri_pipeline);
                pass.set_bind_group(0, &self.uniform_bind, &[]);
                pass.set_vertex_buffer(0, self.tri_buf.slice(..));
                pass.draw(0..self.n_tris, 0..1);
            }
            if self.n_lines > 0 {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.uniform_bind, &[]);
                pass.set_vertex_buffer(0, self.line_buf.slice(..));
                // One instance per segment (both ends, 48 bytes); the vertex shader expands a
                // 6-vertex quad.
                pass.draw(0..6, 0..self.n_lines / 2);
            }
            // Volume rendering (round-211): the volume goes before the transparent bucket, as the
            // farthest transparent layer (ask doc §5.2); the premultiplied output and the t_wall
            // cutoff from the depth pre-pass are done in the shader.
            if let Some((_, bind)) = &volume_draw {
                pass.set_pipeline(&self.volume_pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..36, 0..1);
            }
            // The transparent bucket draws last: depth is read-only (whatever opaque geometry hides
            // is culled by Less), and transparent-on-transparent blends in array order (depth
            // writes are off).
            if self.n_tris_alpha > 0 {
                pass.set_pipeline(&self.tri_alpha_pipeline);
                pass.set_bind_group(0, &self.uniform_bind, &[]);
                pass.set_vertex_buffer(0, self.tri_alpha_buf.slice(..));
                pass.draw(0..self.n_tris_alpha, 0..1);
            }
        }
        self.queue.submit(Some(enc.finish()));
    }

    /// Renders a frame and reads back a BGRA bitmap (`width*height*4`, row padding removed), for
    /// headless tests asserting on pixels (the shell's readback presentation path uses
    /// [`ReadbackTarget`] directly). Blocks for the frame (acceptable, as the viewport renders on
    /// demand; poll(Wait) guarantees the map_async callback fires before returning).
    pub fn render_readback(&mut self, scene: &RenderScene, width: u32, height: u32) -> Option<Vec<u8>> {
        let view = self.ensure_readback(width, height);
        self.render(scene, width, height, &view);
        self.readback.as_ref()?.read(&self.device, &self.queue)
    }
}

/// GPU adapter name (for diagnostics; None without a device).
pub fn adapter_info() -> Option<String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let info = adapter.get_info();
    Some(format!("{} ({})", info.name, backend::label(info.backend)))
}

/// Backend label (wgpu 29's Backend enum has no Display).
mod backend {
    pub fn label(b: wgpu::Backend) -> &'static str {
        match b {
            wgpu::Backend::Vulkan => "Vulkan",
            wgpu::Backend::Metal => "Metal",
            wgpu::Backend::Dx12 => "Dx12",
            wgpu::Backend::Gl => "Gl",
            wgpu::Backend::BrowserWebGpu => "WebGpu",
            _ => "Empty",
        }
    }
}
