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
//!   the host's color view (Windows/Web shells composite that texture via
//!   gpui `paint_surface` — the frame never leaves the GPU);
//!   [`Renderer::render_readback`] maps back a tightly-packed BGRA bitmap
//!   (headless pixel tests / non-Windows shells falling back to
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

/// 渲染顶点：世界坐标 + 法线 + 颜色（三角形与线段共用管线）。color.a
/// 只在不透明桶语义固定为 1；透明桶（[`RenderScene::tris_alpha`]）承
/// 载透明度。
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct RenderVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
}

/// 线段顶点（pos + 颜色；法线朝向相机由着色器处理）。顶点缓冲按
/// **一实例 = 一段的两端**连续排列，顶点着色器把段展开为屏幕空间线宽
/// 四边形（见 [`SHADER_LINE`]）。alpha 恒 1（透明线不在 P0 语义内）。
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct LineVertexGpu {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

/// 相机：正交投影（yaw/pitch 度 + 场域包围盒）。
#[derive(Clone, Copy, Debug)]
pub struct OrthoCamera {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    /// 场域中心（世界坐标）。
    pub center: [f32; 3],
    /// 场域半径（包围球；正交半高 = 半径）。
    pub radius: f32,
}

/// 帧参数（uniform；uniform 地址空间成员须 16 字节对齐）。
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct FrameUniform {
    mvp: [[f32; 4]; 4],
    light_dir: [f32; 3],
    _pad: u32,
    /// xy = 视口尺寸 px；z = 屏幕空间线宽 px（线段展开用）；w = 补齐。
    viewport: [f32; 4],
}

/// 渲染场景（纯数据；产品侧几何构建产出后转入 GPU 顶点）。
pub struct RenderScene {
    /// 不透明桶（depth write on；alpha 语义恒 1）。
    pub tris: Vec<RenderVertex>,
    /// 透明桶（round-205 P0：depth write **OFF**、只读深 `Less`、
    /// `ALPHA_BLENDING`，绘制序 = 数组序——组级 far-to-near 排序是产
    /// 品侧领域知识，桶内逐三角排序是 P1）。头灯着色照旧作用于 rgb，
    /// alpha 只承载透明度。
    pub tris_alpha: Vec<RenderVertex>,
    /// 不透明线（保持）。
    pub lines: Vec<LineVertexGpu>,
    pub camera: OrthoCamera,
    pub bg: [f32; 4],
    /// 屏幕空间线宽（px）：线段展开四边形的整体宽度，随视口分辨率
    /// 保持物理厚度不变。
    pub line_width_px: f32,
}

// ---- 交互相机（视口语义归属渲染核心；GUI 壳只转发裸输入） ----
//
// 轨道灵敏度、平移的世界换算、缩放 clamp 全在本模块；GUI 壳只把鼠标
// 位移/滚轮原始参数递给 [`cam_orbit`]/[`cam_pan`]/[`cam_zoom`]。

/// 视角预设（面板按钮序）：(标签, yaw°, pitch°)。俯视不退化（投影的
/// right 向量只依赖 yaw）。
pub const VIEW_ANGLES: [(&str, f64, f64); 4] = [
    ("等轴", 35.0, 30.0),
    ("前", 0.0, 0.0),
    ("右", 90.0, 0.0),
    ("上", 0.0, 90.0),
];

/// 当前视角序号（会话级视图偏好；面板按钮切换）。
pub static VIEW_ANGLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// 交互相机偏移（会话级，不持久化）：拖拽轨道（yaw/pitch 叠加在视角
/// 预设上）+ 滚轮缩放（radius 乘子）+ Shift 拖拽平移（屏幕空间位移，
/// 世界换算在 [`CamOffset::apply_to`] 按当前视野做）。视角预设按钮重置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamOffset {
    pub yaw: f64,
    pub pitch: f64,
    pub zoom: f64,
    /// 屏幕空间平移累计（px；+x 右 +y 下）。
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
    /// 滚轮缩放（乘子 clamp 0.15..6——约 1.6 个数量级视野）。
    pub fn zoom_by(&mut self, factor: f64) {
        self.zoom = (self.zoom * factor).clamp(0.15, 6.0);
    }

    /// 轨道拖拽（度/像素 × 位移）。
    pub fn orbit_by(&mut self, dx: f64, dy: f64) {
        self.yaw += dx * 0.4;
        self.pitch += dy * 0.4;
    }

    /// 平移拖拽（屏幕像素累计；clamp ±视口几倍防迷失）。
    pub fn pan_by(&mut self, dx: f64, dy: f64) {
        self.pan[0] = (self.pan[0] + dx).clamp(-4000.0, 4000.0);
        self.pan[1] = (self.pan[1] + dy).clamp(-4000.0, 4000.0);
    }

    /// 预设 + 偏移合成最终正交相机（原地改写 yaw/pitch/radius/center）。
    /// 平移换算：屏幕 px → NDC（2×半高/视口高，含 zoom）→ 世界（right/up 基）。
    pub fn apply_to(&self, preset_yaw: f64, preset_pitch: f64, cam: &mut OrthoCamera, view_h_px: f64) {
        let radius = cam.radius as f64; // 基准半径（zoom 前）
        cam.yaw_deg = (preset_yaw + self.yaw) as f32;
        cam.pitch_deg = (preset_pitch + self.pitch) as f32;
        // 世界单位/px：正交半高 = radius×1.2×zoom → NDC 高 2。
        let world_per_px = 2.0 * radius * 1.2 * self.zoom / view_h_px.max(1.0);
        let (cy, sy, cp, sp) = (
            (cam.yaw_deg as f64).to_radians().cos(),
            (cam.yaw_deg as f64).to_radians().sin(),
            (cam.pitch_deg as f64).to_radians().cos(),
            (cam.pitch_deg as f64).to_radians().sin(),
        );
        let r = [-sy, cy, 0.0];
        let u = [-sp * cy, -sp * sy, cp];
        // 拖拽 +x = 相机左移（场景右移）→ center -= r·dx；+y 下 = center += u·dy。
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

/// 壳层裸输入 → 轨道拖拽（位移 px；灵敏度语义在本模块）。
pub fn cam_orbit(dx: f64, dy: f64) {
    CAM.with(|c| c.borrow_mut().orbit_by(dx, dy));
}

/// 壳层裸输入 → Shift 平移拖拽（位移 px）。
pub fn cam_pan(dx: f64, dy: f64) {
    CAM.with(|c| c.borrow_mut().pan_by(dx, dy));
}

/// 壳层裸输入 → 滚轮缩放（乘子）。
pub fn cam_zoom(factor: f64) {
    CAM.with(|c| c.borrow_mut().zoom_by(factor));
}

/// 预设 + 偏移合成最终相机（视角预设切换时先 [`reset_cam_offset`]）。
pub fn cam_apply(preset_yaw: f64, preset_pitch: f64, cam: &mut OrthoCamera, view_h_px: f64) {
    CAM.with(|c| c.borrow().apply_to(preset_yaw, preset_pitch, cam, view_h_px));
}

/// 重置相机偏移（视角预设切换时调用）。
pub fn reset_cam_offset() {
    CAM.with(|c| *c.borrow_mut() = CamOffset::default());
}

/// 相机偏移当前值（测试访问器）。
pub fn cam_offset_for_test() -> CamOffset {
    CAM.with(|c| *c.borrow())
}

/// 列主序 4×4。
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

/// 正交 MVP（view-lookat + ortho；相机在 (0,0,3r) 看向中心，先转 yaw/pitch）。
fn ortho_mvp(cam: &OrthoCamera, aspect: f32) -> Mat4 {
    let (yaw, pitch) = (cam.yaw_deg.to_radians(), cam.pitch_deg.to_radians());
    let (cy, sy, cp, sp) = (yaw.cos(), yaw.sin(), pitch.cos(), pitch.sin());
    // 相机基（右手，up=+z 世界）：d 指向相机、r 右、u 上。
    let d = [cp * cy, cp * sy, sp];
    let r = [-sy, cy, 0.0];
    let u = [-sp * cy, -sp * sy, cp];
    let eye = [
        cam.center[0] + d[0] * cam.radius * 3.0,
        cam.center[1] + d[1] * cam.radius * 3.0,
        cam.center[2] + d[2] * cam.radius * 3.0,
    ];
    // view = [r; u; -d] 平移（行主序构造后转列主序）。
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
    // 正交投影（半高 = 1.2×半径留边；near/far 覆盖相机前 [0.01r, 8r]）。
    // **WebGPU NDC 深度是 [0,1]**（非 OpenGL [-1,1]）：z_ndc = (z-n)/(f-n)。
    let hh = cam.radius * 1.2;
    let hw = hh * aspect.max(1e-3);
    let (n, f) = (0.01 * cam.radius.max(1e-6), cam.radius * 8.0);
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
    viewport: vec4<f32>, // 三角形管线不用（与线段管线共用 uniform 布局）。
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
    // 头灯：0.45 环境 + 0.55 |n·l|（明暗乘进 rgb，alpha 只承载透明度
    // ——透明桶管线用 ALPHA_BLENDING，不透明桶 REPLACE 下 a=1 等价现状）。
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
    viewport: vec4<f32>, // xy = 视口 px；z = 屏幕空间线宽 px。
};
@group(0) @binding(0) var<uniform> frame: FrameUniform;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// 六顶点角表（两三角形拼成一段的四边形）：x = 沿段参数 t，y = 垂直符号。
fn corner(vi: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    return c[vi];
}

// 屏幕空间线宽：一实例读一段的两端，先各自过 MVP 到 clip 空间，再在
// 屏幕 px 域取垂直方向扩张半宽（NDC→px→NDC 换算含视口尺寸，物理厚度
// 不随分辨率变）；深度按角点参数在两端 NDC 深度间线性插值——正交投影
// w=1 恒定，遮挡与 1px LineList 一致。
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
    // 线方向 NDC→px（px = ndc × 0.5×视口），取屏幕空间垂直单位向量。
    let to_px = 0.5 * frame.viewport.xy;
    let d = (nb - na) * to_px;
    let len = length(d);
    let perp = select(vec2<f32>(1.0, 0.0), vec2<f32>(-d.y, d.x) / len, len > 1e-6);
    // 半宽 px → NDC（逐分量 ÷ 半视口 px），角点沿垂直方向扩张。
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

/// 纯 wgpu 渲染核心：只依赖 wgpu 类型——设备/队列/目标格式由宿主递入
/// （GUI 壳递共享设备走 surface 合成；headless 测试自建本地设备），
/// 本模块不创建窗口、不触碰任何 GUI 类型。
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,
    tri_pipeline: wgpu::RenderPipeline,
    tri_alpha_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    tri_buf: wgpu::Buffer,
    tri_alpha_buf: wgpu::Buffer,
    line_buf: wgpu::Buffer,
    n_tris: u32,
    n_tris_alpha: u32,
    n_lines: u32,
    /// 深度目标（两条呈现路径共用；随尺寸重建；MSAA 倍数同颜色）。
    depth: Option<(wgpu::TextureView, u32, u32)>,
    /// MSAA 颜色目标（resolve 到宿主传入的颜色 view；随尺寸重建）。
    msaa: Option<(wgpu::TextureView, u32, u32)>,
    /// 回读目标（`render_readback` 专用：颜色纹理 + 读回缓冲）。
    readback: Option<ReadbackTarget>,
}

struct ReadbackTarget {
    texture: wgpu::Texture,
    buf: wgpu::Buffer,
    width: u32,
    height: u32,
    bytes_per_row: u32,
}

impl Renderer {
    /// MSAA 倍数（4x——WebGPU 规范对可渲染格式保证支持）。
    const MSAA_SAMPLES: u32 = 4;

    /// 共享宿主设备构造（无失败路径：设备/队列由宿主保证可用）。
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
            // 线段管线：一实例 = 一段的两端（连续 2×LineVertexGpu），
            // 顶点着色器展开 6 顶点四边形；三角形管线逐顶点。两者同为
            // TriangleList + 深度 + MSAA。透明三角管线（alpha=true）：
            // ALPHA_BLENDING + depth write OFF（只读深 Less）——绘制序
            // 即混合序，排序语义见 RenderScene::tris_alpha 文档。
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
                    buffers: &vb_layout,
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

    /// MSAA 颜色目标就绪（resolve 落到宿主 view；尺寸不变即复用）。
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

    /// 回读目标就绪（颜色纹理 + 256 对齐读回缓冲；尺寸不变即复用）。
    fn ensure_readback(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        let (w, h) = (width.max(1), height.max(1));
        if let Some(t) = &self.readback {
            if t.width == w && t.height == h {
                return t.texture.create_view(&Default::default());
            }
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wb-render-target"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let bytes_per_row = (w * 4).next_multiple_of(256);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wb-render-readback"),
            size: bytes_per_row as u64 * h as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = texture.create_view(&Default::default());
        self.readback = Some(ReadbackTarget {
            texture,
            buf,
            width: w,
            height: h,
            bytes_per_row,
        });
        view
    }

    /// 渲一帧到宿主颜色目标：mvp/头灯 uniform 更新 + 顶点上传 + 绘制编码
    /// （纹理全程留 GPU——壳层 `paint_surface` 合成；视口按需渲染，非每帧
    /// 热路径）。
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
        // 相机看向场景的方向 = 光方向（头灯）。
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
        // 顶点上传（必要时扩容）。
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
        let need_tri_alpha = (scene.tris_alpha.len() * size_of::<RenderVertex>()) as u64;
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
        if !scene.tris_alpha.is_empty() {
            self.queue.write_buffer(
                &self.tri_alpha_buf,
                0,
                bytemuck::cast_slice(&scene.tris_alpha),
            );
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

        let depth_view = self.ensure_depth(width, height);
        let msaa_view = self.ensure_msaa(width, height);
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wb-render-enc"),
            });
        {
            let bg = wgpu::LoadOp::Clear(wgpu::Color {
                r: scene.bg[0] as f64,
                g: scene.bg[1] as f64,
                b: scene.bg[2] as f64,
                a: scene.bg[3] as f64,
            });
            // 渲进 MSAA 附件，resolve 落到宿主颜色 view（paint_surface 的
            // 离屏纹理 / readback 纹理均为 RENDER_ATTACHMENT 用法）。
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
                // 一实例一段（两端 48 字节），顶点着色器展开 6 顶点四边形。
                pass.draw(0..6, 0..self.n_lines / 2);
            }
            // 透明桶最后画：只读深（被不透明几何遮挡者被 Less 剔除），
            // 透明-透明按数组序混合（深度写已关）。
            if self.n_tris_alpha > 0 {
                pass.set_pipeline(&self.tri_alpha_pipeline);
                pass.set_bind_group(0, &self.uniform_bind, &[]);
                pass.set_vertex_buffer(0, self.tri_alpha_buf.slice(..));
                pass.draw(0..self.n_tris_alpha, 0..1);
            }
        }
        self.queue.submit(Some(enc.finish()));
    }

    /// 渲一帧并回读 BGRA 位图（`width*height*4`，行已去 padding）——
    /// headless 测试断言像素 / 非 Windows 壳的 `paint_image` 回退路径。
    /// 阻塞等帧（视口按需渲染，可接受；poll(Wait) 保证 map_async 回调
    /// 在返回前触发）。
    pub fn render_readback(&mut self, scene: &RenderScene, width: u32, height: u32) -> Option<Vec<u8>> {
        let view = self.ensure_readback(width, height);
        self.render(scene, width, height, &view);
        let t = self.readback.as_ref()?;
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wb-render-readback-enc"),
            });
        enc.copy_texture_to_buffer(
            t.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &t.buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(t.bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: t.width,
                height: t.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(enc.finish()));
        let (sx, rx) = std::sync::mpsc::channel();
        let buf = t.buf.clone();
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = sx.send(res);
        });
        self.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
            .ok()?;
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().to_vec();
        buf.unmap();
        // 去行 padding。
        let w = t.width as usize;
        let h = t.height as usize;
        let bpr = t.bytes_per_row as usize;
        let mut out = Vec::with_capacity(w * h * 4);
        for row in 0..h {
            let s = row * bpr;
            out.extend_from_slice(&data[s..s + w * 4]);
        }
        Some(out)
    }
}

/// GPU 适配器名（诊断用；无设备 None）。
pub fn adapter_info() -> Option<String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let info = adapter.get_info();
    Some(format!("{} ({})", info.name, backend::label(info.backend)))
}

/// 后端标签（wgpu 29 的 Backend 枚举无 Display）。
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
