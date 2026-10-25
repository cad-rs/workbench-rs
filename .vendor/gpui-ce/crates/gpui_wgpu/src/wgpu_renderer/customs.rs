//! `Primitive::Custom` 扩展点：产品自定义 wgpu 管线在同一主 pass、交换链
//! 目标上按 UI 绘制顺序执行——无离屏纹理、无表面合成（gpui 侧类型擦除，
//! 此处下转型执行）。主 pass 是纯 color pass（无深度附件）：适合透明
//! 叠加 / 2D 特效 / 程序化内容；带深度测试的 3D 内容仍走 paint_surface
//! 离屏合成。非 wgpu 后端（Metal 等）跳过并记日志，与
//! `SurfaceSource::Texture` 的降级契约一致。

use gpui::PaintCustom;

use super::{frame, WgpuRenderer};

/// gpui `PaintCustom::data` 的具体载荷（`Arc<CustomDraw>` 装入
/// `Arc<dyn Any + Send + Sync>`，绘制时由本模块下转型）。
#[derive(Clone)]
pub struct CustomDraw {
    /// 绘制回调：调用前 pass 的 scissor 已设为本原语的裁剪矩形；env
    /// 提供设备/队列/目标尺寸。管线/顶点/uniform 由回调自管（建议经
    /// `Arc` 捕获构建一次、逐帧复用，避免每帧重建 GPU 资源）。
    pub draw: std::sync::Arc<dyn Fn(&mut wgpu::RenderPass<'_>, &CustomDrawEnv) + Send + Sync>,
}

/// 回调环境（全部设备像素口径）。
#[derive(Clone)]
pub struct CustomDrawEnv {
    /// 渲染器共享设备（回调可在此创建/更新自己的 GPU 资源）。
    pub device: wgpu::Device,
    /// 渲染器共享队列（回调可 write_buffer 提交本帧数据）。
    pub queue: wgpu::Queue,
    /// 目标纹理尺寸 (width, height)。
    pub target_size: (u32, u32),
    /// 目标纹理格式（回调建管线时作 ColorTargetState.format）。
    pub target_format: wgpu::TextureFormat,
    /// bounds ∩ content_mask 的裁剪矩形 (x, y, w, h)——pass 的 scissor
    /// 已设为此值；这是 UI 裁剪契约，回调不应清除或放大它。
    pub clip: (u32, u32, u32, u32),
}

pub(in crate::wgpu_renderer) fn draw_customs(
    renderer: &WgpuRenderer,
    customs: &[PaintCustom],
    pass: &mut wgpu::RenderPass<'_>,
) -> frame::DrawResult {
    let resources = renderer.resources();
    let env = CustomDrawEnv {
        device: (*resources.device).clone(),
        queue: (*resources.queue).clone(),
        target_size: (renderer.target.width(), renderer.target.height()),
        target_format: renderer.target.format(),
        clip: (0, 0, 0, 0),
    };
    for custom in customs {
        let Some(draw) = custom.data.downcast_ref::<CustomDraw>() else {
            log::error!("custom primitive payload is not gpui_wgpu::CustomDraw; skipping");
            continue;
        };
        let clipped = custom.bounds.intersect(&custom.content_mask.bounds);
        // wgpu scissor 全 u32：先钳非负，再钳到目标内。
        let x = clipped.origin.x.0.max(0.0).floor() as u32;
        let y = clipped.origin.y.0.max(0.0).floor() as u32;
        let mut clip = (
            x.min(env.target_size.0),
            y.min(env.target_size.1),
            clipped.size.width.0.max(0.0).floor() as u32,
            clipped.size.height.0.max(0.0).floor() as u32,
        );
        clip.2 = clip.2.min(env.target_size.0.saturating_sub(clip.0));
        clip.3 = clip.3.min(env.target_size.1.saturating_sub(clip.1));
        if clip.2 == 0 || clip.3 == 0 {
            continue;
        }
        pass.set_scissor_rect(clip.0, clip.1, clip.2, clip.3);
        (draw.draw)(&mut *pass, &CustomDrawEnv { clip, ..env.clone() });
    }
    Ok(())
}

#[cfg(all(test, feature = "test-support", not(target_family = "wasm")))]
mod tests {
    use super::*;
    use gpui::{point, size, ContentMask, DevicePixels, ScaledPixels, Scene, Size};
    use std::sync::OnceLock;

    /// 自定义 pass 扩展点 e2e（headless）：`PaintCustom` 载荷下转型执行，
    /// 自定义管线（顶点无缓冲大三角 + 实色 fragment）写入目标，scissor
    /// 裁剪生效——矩形内绿、矩形外保持清除色。
    #[test]
    fn custom_draw_writes_pixels_within_scissor() -> anyhow::Result<()> {
        let context = crate::WgpuContext::new_headless(None)?;
        let target_size = Size {
            width: DevicePixels(40),
            height: DevicePixels(20),
        };
        let mut renderer = crate::WgpuRenderer::new_headless(&context, target_size)?;

        let bounds = gpui::Bounds {
            origin: point(ScaledPixels(10.0), ScaledPixels(5.0)),
            size: size(ScaledPixels(20.0), ScaledPixels(10.0)),
        };
        let payload = std::sync::Arc::new(CustomDraw {
            draw: std::sync::Arc::new(|pass: &mut wgpu::RenderPass<'_>, env: &CustomDrawEnv| {
                static PIPELINE: OnceLock<wgpu::RenderPipeline> = OnceLock::new();
                let pipeline = PIPELINE.get_or_init(|| {
                    let module = env.device.create_shader_module(
                        wgpu::ShaderModuleDescriptor {
                            label: Some("custom-draw-test"),
                            source: wgpu::ShaderSource::Wgsl(
                                r#"
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2(-3.0, -3.0), vec2(5.0, -3.0), vec2(-3.0, 5.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4(0.0, 1.0, 0.0, 1.0);
}
"#
                                .into(),
                        ),
                    });
                    env.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("custom-draw-test-pipeline"),
                        layout: Some(&env.device.create_pipeline_layout(
                            &wgpu::PipelineLayoutDescriptor {
                                label: None,
                                bind_group_layouts: &[],
                                immediate_size: 0,
                            },
                        )),
                        vertex: wgpu::VertexState {
                            module: &module,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &module,
                            entry_point: Some("fs_main"),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format: env.target_format,
                                blend: Some(wgpu::BlendState::REPLACE),
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        primitive: Default::default(),
                        depth_stencil: None,
                        multisample: Default::default(),
                        multiview_mask: None,
                        cache: None,
                    })
                });
                pass.set_pipeline(pipeline);
                pass.draw(0..3, 0..1);
            }),
        });

        let mut scene = Scene::default();
        scene.insert_primitive(PaintCustom {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            data: payload,
        });
        scene.finish();

        let image = renderer.render_to_image(&scene)?;
        // 矩形内（(10,5)+(20,10) 的中心）= 绿；矩形外 = headless 清除色
        //（白）——scissor 之外的像素未被自定义管线触碰。render_to_image
        // 是 RGBA。
        let inside = image.get_pixel(20, 10).0;
        assert_eq!(inside, [0, 255, 0, 255], "custom draw fills the clip rect");
        let outside = image.get_pixel(39, 0).0;
        assert_eq!(
            outside,
            [255, 255, 255, 255],
            "outside the scissor stays the clear color"
        );
        Ok(())
    }
}
