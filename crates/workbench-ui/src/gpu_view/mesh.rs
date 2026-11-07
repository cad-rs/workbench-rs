//! Simple parametric meshes: boxes and cylinders, flat shaded (per-face normals),
//! plus the "feature edge" extraction common in CAD (polygon edges whose adjacent faces meet at an
//! angle above a threshold).
//! Ported from the gpui-wgpu reference implementation (javagg/gpui-demo).

use glam::Vec3;
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SolidVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

#[derive(Default, Clone)]
pub struct Mesh {
    pub verts: Vec<SolidVertex>,
}

fn quant(p: Vec3) -> [i64; 3] {
    [
        (p.x * 64.0).round() as i64,
        (p.y * 64.0).round() as i64,
        (p.z * 64.0).round() as i64,
    ]
}

impl Mesh {
    fn push_tri(&mut self, a: Vec3, b: Vec3, c: Vec3, color: Vec3) {
        let normal = (b - a).cross(c - a).normalize_or_zero();
        for p in [a, b, c] {
            self.verts.push(SolidVertex {
                pos: p.to_array(),
                normal: normal.to_array(),
                color: color.to_array(),
            });
        }
    }

    fn push_quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: Vec3) {
        self.push_tri(a, b, c, color);
        self.push_tri(a, c, d, color);
    }

    /// Axis-aligned box
    pub fn box_mesh(center: Vec3, size: Vec3, color: Vec3) -> Self {
        let h = size * 0.5;
        // Compute the 8 corners only once so shared vertices are numerically identical
        // (feature edge extraction relies on this)
        let c = |sx: f32, sy: f32, sz: f32| center + Vec3::new(sx * h.x, sy * h.y, sz * h.z);
        let p = [
            c(-1.0, -1.0, -1.0),
            c(1.0, -1.0, -1.0),
            c(1.0, -1.0, 1.0),
            c(-1.0, -1.0, 1.0),
            c(-1.0, 1.0, -1.0),
            c(1.0, 1.0, -1.0),
            c(1.0, 1.0, 1.0),
            c(-1.0, 1.0, 1.0),
        ];
        let mut m = Mesh::default();
        // bottom / top / -Z / +Z / -X / +X
        m.push_quad(p[0], p[3], p[2], p[1], color);
        m.push_quad(p[4], p[5], p[6], p[7], color);
        m.push_quad(p[0], p[1], p[5], p[4], color);
        m.push_quad(p[3], p[7], p[6], p[2], color);
        m.push_quad(p[0], p[4], p[7], p[3], color);
        m.push_quad(p[1], p[2], p[6], p[5], color);
        m
    }

    /// A Y-up cylinder; `base` is the center of the bottom face
    pub fn cylinder_y(base: Vec3, radius: f32, height: f32, segments: usize, color: Vec3) -> Self {
        let ring = ring_points(base, radius, height, segments);
        let mut m = Mesh::default();
        for i in 0..segments {
            let j = (i + 1) % segments;
            // sides
            m.push_quad(ring[i].0, ring[j].0, ring[j].1, ring[i].1, color);
            // top and bottom caps
            m.push_tri(base, ring[j].0, ring[i].0, color);
            let top_c = base + Vec3::Y * height;
            m.push_tri(top_c, ring[i].1, ring[j].1, color);
        }
        m
    }

    /// Append another mesh
    pub fn merge(&mut self, other: &Mesh) {
        self.verts.extend_from_slice(&other.verts);
    }
}

/// Returns the ring points, each item being (lower ring point, upper ring point)
fn ring_points(base: Vec3, radius: f32, height: f32, segments: usize) -> Vec<(Vec3, Vec3)> {
    let mut ring = Vec::with_capacity(segments);
    for i in 0..segments {
        let a = (i as f32) / (segments as f32) * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        let offset = Vec3::new(c * radius, 0.0, s * radius);
        ring.push((base + offset, base + offset + Vec3::Y * height));
    }
    ring
}

/// Feature edge extraction: judged by the angle between the normals on the two sides of a
/// shared edge.
/// Returns a flattened list of segment endpoints (two points per edge).
pub fn feature_edges(mesh: &Mesh, cos_threshold: f32) -> Vec<LineVertex> {
    #[derive(Default)]
    struct EdgeInfo {
        normals: Vec<Vec3>,
        a: Vec3,
        b: Vec3,
    }
    let mut edges: HashMap<([i64; 3], [i64; 3]), EdgeInfo> = HashMap::new();

    for tri in mesh.verts.as_chunks::<3>().0 {
        let pts = [
            Vec3::from(tri[0].pos),
            Vec3::from(tri[1].pos),
            Vec3::from(tri[2].pos),
        ];
        let n = Vec3::from(tri[0].normal);
        for i in 0..3 {
            let p = pts[i];
            let q = pts[(i + 1) % 3];
            let kp = quant(p);
            let kq = quant(q);
            let (lo_k, hi_k, lo_p, hi_p) = if kp <= kq {
                (kp, kq, p, q)
            } else {
                (kq, kp, q, p)
            };
            let entry = edges.entry((lo_k, hi_k)).or_default();
            if entry.normals.is_empty() {
                entry.a = lo_p;
                entry.b = hi_p;
            }
            entry.normals.push(n);
        }
    }

    let mut out = Vec::new();
    let dark = [0.05, 0.05, 0.07, 1.0];
    for (_, info) in edges {
        let mut is_feature = info.normals.len() < 2; // boundary edges count as feature edges too
        if !is_feature {
            'outer: for i in 0..info.normals.len() {
                for j in (i + 1)..info.normals.len() {
                    if info.normals[i].dot(info.normals[j]) < cos_threshold {
                        is_feature = true;
                        break 'outer;
                    }
                }
            }
        }
        if is_feature {
            out.push(LineVertex {
                pos: info.a.to_array(),
                color: dark,
            });
            out.push(LineVertex {
                pos: info.b.to_array(),
                color: dark,
            });
        }
    }
    out
}

/// Grid lines on the XZ plane
pub fn grid_lines(span: f32, step: f32, color: [f32; 4]) -> Vec<LineVertex> {
    let mut out = Vec::new();
    let mut v = |a: Vec3, b: Vec3| {
        out.push(LineVertex {
            pos: a.to_array(),
            color,
        });
        out.push(LineVertex {
            pos: b.to_array(),
            color,
        });
    };
    let mut t = -span;
    while t <= span + 0.001 {
        v(Vec3::new(t, 0.0, -span), Vec3::new(t, 0.0, span));
        v(Vec3::new(-span, 0.0, t), Vec3::new(span, 0.0, t));
        t += step;
    }
    out
}

/// Coordinate axes: X red, Z blue, Y green
pub fn axes_lines(len: f32) -> Vec<LineVertex> {
    let mut out = Vec::new();
    let mut seg = |a: Vec3, b: Vec3, c: [f32; 4]| {
        out.push(LineVertex {
            pos: a.to_array(),
            color: c,
        });
        out.push(LineVertex {
            pos: b.to_array(),
            color: c,
        });
    };
    seg(
        Vec3::ZERO,
        Vec3::new(len, 0.0, 0.0),
        [0.90, 0.25, 0.25, 1.0],
    );
    seg(
        Vec3::ZERO,
        Vec3::new(0.0, len * 0.6, 0.0),
        [0.30, 0.85, 0.35, 1.0],
    );
    seg(
        Vec3::ZERO,
        Vec3::new(0.0, 0.0, len),
        [0.30, 0.50, 0.95, 1.0],
    );
    out
}
