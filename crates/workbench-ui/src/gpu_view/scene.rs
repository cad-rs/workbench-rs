//! wgpu 渲染核心：自定义 WGSL shader、实体/线段两条管线、
//! 场景缓冲与 uniform；渲染目标由 `present` 按平台提供。
//! 移植自 gpui-wgpu 参考实现（javagg/gpui-demo）。

use super::mesh::{self, LineVertex, Mesh, SolidVertex};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

const SHADER_SRC: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,     // xyz = 相机位置, w = 线段深度偏移
    light: vec4<f32>,   // xyz = 光照方向
};
@group(0) @binding(0) var<uniform> u: Uniforms;

// ---- 实体管线 ----
struct SolidIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};
struct SolidOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_solid(inp: SolidIn) -> SolidOut {
    var out: SolidOut;
    out.clip = u.view_proj * vec4<f32>(inp.pos, 1.0);
    out.normal = inp.normal;
    out.color = inp.color;
    out.world = inp.pos;
    return out;
}

@fragment
fn fs_solid(inp: SolidOut) -> @location(0) vec4<f32> {
    let n = normalize(inp.normal);
    let l = normalize(u.light.xyz);
    let v = normalize(u.eye.xyz - inp.world);
    let h = normalize(l + v);
    var lambert = max(dot(n, l), 0.0);
    if (lambert <= 0.0) {
        // 双面补光，避免背光面死黑
        lambert = max(dot(-n, l), 0.0) * 0.30;
    }
    let spec = pow(max(dot(n, h), 0.0), 40.0) * 0.22;
    let col = inp.color * (0.30 + 0.70 * lambert) + vec3<f32>(spec);
    return vec4<f32>(col, 1.0);
}

// ---- 线段管线 ----
struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};
struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(inp: LineIn) -> LineOut {
    var clip = u.view_proj * vec4<f32>(inp.pos, 1.0);
    clip.z -= u.eye.w * clip.w; // 轻微向视点方向偏移，避免与实体面深度冲突
    var out: LineOut;
    out.clip = clip;
    out.color = inp.color;
    return out;
}

@fragment
fn fs_line(inp: LineOut) -> @location(0) vec4<f32> {
    return inp.color;
}
"#;

/// 每帧的全局 uniform（WGSL 对齐：mat4 + 2×vec4 = 192 字节）
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub view_proj: [[f32; 4]; 4],
    pub eye: [f32; 4],
    pub light: [f32; 4],
}

impl Uniforms {
    pub fn new(view_proj: Mat4, eye: Vec3, light: Vec3, line_bias: f32) -> Self {
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            eye: [eye.x, eye.y, eye.z, line_bias],
            light: [light.x, light.y, light.z, 0.0],
        }
    }
}

/// 每个视口私有的 uniform 缓冲与 bind group
pub struct ViewState {
    pub uniform_buf: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}

impl ViewState {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport-uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport-bind-group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        Self {
            uniform_buf,
            bind_group,
        }
    }
}

pub struct Scene {
    pub solid_vb: wgpu::Buffer,
    pub line_vb: wgpu::Buffer,
    pub solid_verts: u32,
    pub line_verts: u32,
}

impl Scene {
    /// 构建演示零件：底板 + 中心凸台 + 四个定位销，并生成网格、特征边、参考网格线。
    pub fn new(device: &wgpu::Device) -> Self {
        let plate_color = Vec3::new(0.58, 0.61, 0.66);
        let boss_color = Vec3::new(0.66, 0.60, 0.52);
        let pin_color = Vec3::new(0.72, 0.55, 0.30);

        let mut model = Mesh::default();

        // 底板 140 × 16 × 90
        let plate = Mesh::box_mesh(Vec3::new(0.0, 8.0, 0.0), Vec3::new(140.0, 16.0, 90.0), plate_color);
        model.merge(&plate);

        // 中心凸台 Ø56 × 46
        let boss = Mesh::cylinder_y(Vec3::new(0.0, 16.0, 0.0), 28.0, 46.0, 48, boss_color);
        model.merge(&boss);

        // 四个定位销 Ø10 × 10
        for (sx, sz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let pin = Mesh::cylinder_y(
                Vec3::new(sx * 55.0, 16.0, sz * 30.0),
                5.0,
                10.0,
                24,
                pin_color,
            );
            model.merge(&pin);
        }

        // 特征边（法线夹角 > 30° 或边界边）
        let mut lines: Vec<LineVertex> =
            mesh::feature_edges(&model, (30f32).to_radians().cos());
        // 参考网格与坐标轴
        lines.extend(mesh::grid_lines(200.0, 20.0, [0.35, 0.38, 0.45, 0.55]));
        lines.extend(mesh::axes_lines(140.0));

        Self::from_mesh(device, &model, &lines)
    }

    /// 产品注入入口（注入 API）：自任意 `Mesh`（顶点即三角形扇）+ 特征
    /// 线构建场景。`Scene::new`（演示零件）经由此委托——上传路径单一来源。
    pub fn from_mesh(device: &wgpu::Device, model: &Mesh, lines: &[LineVertex]) -> Self {
        let solid_bytes = bytemuck::cast_slice::<SolidVertex, u8>(&model.verts);
        let line_bytes = bytemuck::cast_slice::<LineVertex, u8>(lines);

        let solid_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene-solid-vb"),
            contents: solid_bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let line_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene-line-vb"),
            contents: line_bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            solid_vb,
            line_vb,
            solid_verts: model.verts.len() as u32,
            line_verts: lines.len() as u32,
        }
    }
}

pub struct Pipelines {
    pub layout: wgpu::BindGroupLayout,
    pub solid: wgpu::RenderPipeline,
    pub line: wgpu::RenderPipeline,
}

impl Pipelines {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cad-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cad-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cad-pipeline-layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let solid_vbl = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SolidVertex>() as u64,
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
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 24,
                    shader_location: 2,
                },
            ],
        };

        let line_vbl = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
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
            ],
        };

        let depth = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24Plus,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        };

        let solid = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cad-solid-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_solid"),
                compilation_options: Default::default(),
                buffers: &[Some(solid_vbl)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_solid"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let line = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cad-line-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[Some(line_vbl)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_line"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            layout,
            solid,
            line,
        }
    }
}

/// 在给定的 render pass 里画出场景（实体 + 线段）。
/// 桌面端与 web 端共用。
pub fn draw_scene(
    pass: &mut wgpu::RenderPass<'_>,
    scene: &Scene,
    pipes: &Pipelines,
    bind_group: &wgpu::BindGroup,
) {
    pass.set_bind_group(0, bind_group, &[]);

    // 实体
    pass.set_pipeline(&pipes.solid);
    pass.set_vertex_buffer(0, scene.solid_vb.slice(..));
    pass.draw(0..scene.solid_verts, 0..1);

    // 线段（网格 / 坐标轴 / 特征边）
    pass.set_pipeline(&pipes.line);
    pass.set_vertex_buffer(0, scene.line_vb.slice(..));
    pass.draw(0..scene.line_verts, 0..1);
}

