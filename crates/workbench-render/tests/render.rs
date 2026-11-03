//! workbench-render 渲染核心测试（headless——无窗口跑 GPU 管线；无可用
//! 适配器的环境静默跳过）。渲染核心的设备由宿主递入（生产契约）：测试
//! 自建本地设备，`render_readback` 回读位图断言像素。

use workbench_render::{
    CamOffset, LineVertexGpu, OrthoCamera, RenderScene, RenderVertex, Renderer,
    cam_offset_for_test, reset_cam_offset,
};

/// 自建本地 headless 设备 + 渲染器（等价生产壳递共享设备的契约）。
fn headless_renderer() -> Option<Renderer> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wb-render-test"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: Default::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    Some(Renderer::new(device, queue, wgpu::TextureFormat::Bgra8Unorm))
}

fn cam() -> OrthoCamera {
    // 俯视（相机在 +z 上方）：正对 x-y 平面的被测几何。
    OrthoCamera {
        yaw_deg: 0.0,
        pitch_deg: 90.0,
        center: [0.0, 0.0, 0.0],
        radius: 1.0,
    }
}

/// 红色三角形占满视口中心：中心像素红色、角落背景色；尺寸/padding 去除。
#[test]
fn renders_triangle_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
        return;
    };
    let red = [1.0, 0.0, 0.0, 1.0];
    let v = |x: f32, y: f32| RenderVertex {
        pos: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: red,
    };
    let scene = RenderScene {
        // 中等三角形（NDC ±0.42）：中心在内、四角在外。
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
    // readback 是 BGRA（纹理 Bgra8Unorm，与 gpui RenderImage 对齐）。
    let px = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        (bytes[i + 2], bytes[i + 1], bytes[i]) // (R, G, B)
    };
    let (r_, g, b) = px(32, 32);
    // 头灯明暗：法线正对相机 → shade ≈ 1.0，红色近满。
    assert!(r_ > 150, "center red, got {r_} {g} {b}");
    assert!(g < 60 && b < 60, "center not green/blue");
    // 角落在三角形外（顶点 (0,2) 只到屏幕上缘中部）→ 背景。
    let (br, bg, bb) = px(2, 2);
    assert!(br < 30 && bg < 30 && bb < 30, "corner bg, got {br} {bg} {bb}");
    // MSAA 4x：斜边必有半覆盖像素（红与背景的混合）——抗锯齿生效。
    let mixed = (0..(w * h) as usize)
        .filter(|&i| {
            let r = bytes[i * 4 + 2];
            r > 1 && r < 254 && bytes[i * 4 + 1] < 60
        })
        .count();
    assert!(mixed > 0, "MSAA blends edge pixels, got {mixed} mixed");
}

/// 线段管线：pitch=90 俯视下 x 轴线段 → 屏幕竖线（right=+y → x_screen 恒
/// 中列；线长 ±1.5 超出半高 1.2 → 贯穿全屏）——列 32 各行绿、列 2 无绿。
/// 1.5px 展开四边形跨列 31/32 各 75% 覆盖（MSAA 解析后 g≈191）。
#[test]
fn renders_line_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
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
    // 列 c 的绿像素数（竖线判定）。
    let green_in_column = |c: usize| -> usize {
        (0..h as usize)
            .filter(|&y| bytes[(y * w as usize + c) * 4 + 1] > 100)
            .count()
    };
    // 竖线在视口正中（NDC x=0 恰在像素 31/32 边界，1.5px 宽跨两列各
    // 75%）；旁列零。
    let n_mid = green_in_column(31) + green_in_column(32);
    assert!(n_mid > 30, "center column has the line, got {n_mid}");
    assert_eq!(green_in_column(2), 0, "side column stays background");
}

/// 屏幕空间线宽：8px 线段四边形在 64px 与 128px 视口下均保持 8 列物理
/// 厚度（px 语义而非 NDC 拉伸——分辨率加倍厚度不变）。
#[test]
fn line_screen_space_width_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
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
    // 64px：NDC x=0 在像素 31/32 边界，8px 宽 → 列 28..35 覆盖。
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
    // 128px：中心在 63/64 边界，仍 8 物理像素 → 列 60..67。
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

/// 线段深度正确性：四边形角点深度按段参数线性插值——不透明墙遮挡墙后
/// 部分；斜穿深度分界的线段「墙后隐、墙前显」（角点插值错则分界错位）。
#[test]
fn line_depth_occlusion_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
        return;
    };
    // 「前」视（yaw 0 pitch 0）：视线沿 -x，屏幕右 = +y，深度 = 世界 x
    // （越大越近相机）。
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
    // 覆盖全视口的红墙（±3 覆盖 NDC ±1.2），位于给定深度 x。
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
    // 场景 A：线整体在墙后（x=-0.5 恒 < 0）→ 全帧无绿。
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
    // 场景 B：线斜穿深度（t=0 端 x=-1 在墙后、t=1 端 x=+1 在墙前），
    // 墙 x=0.3 → 深度分界在段参数 0.65（列 ≈44）；其右可见、其左被墙
    // 挡住。若角点深度未按 t 插值（整段取端点深度），分界会错到全隐。
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
    // 墙未覆盖的右缘（列 61+，NDC x > 0.833）线也可见。
    assert!(col(62) > 0, "beyond wall edge visible");
}

/// 交互相机偏移：轨道累加（度/像素）、缩放乘子 clamp、重置。
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
    // 重置语义（视角预设按钮）。
    reset_cam_offset();
    assert_eq!(cam_offset_for_test(), CamOffset::default());
    // 平移累计与 clamp（±4000px 防迷失）。
    let mut c = CamOffset::default();
    c.pan_by(100.0, -200.0);
    assert_eq!(c.pan, [100.0, -200.0]);
    c.pan_by(9000.0, -9000.0);
    assert_eq!(c.pan, [4000.0, -4000.0], "pan clamps");
}

/// 适配器诊断信息可取得（无 GPU 环境为 None，不 panic）。
#[test]
fn adapter_info_does_not_panic() {
    let _ = workbench_render::adapter_info();
}

/// 透明桶质心排序（round-206 P1，`sort_alpha` 声明式开关）：①开关关 =
/// P0 语义——数组序 = 混合序，近面先声明时被远面主导（组内 artifact
/// 实测编码：中心 b=128 ≈ 近绿 g=64 两倍，theory (64,64,128)）；②开关
/// 开 = 质心 far-to-near 重排，近面后画主导（(63,128,64)）；③声明序互
/// 换后逐字节一致——排序消灭声明序语义（凸双三角排序正确性强断言）。
/// 前视（yaw 0 pitch 0）：深度 = 世界 x，越大越近相机。
#[test]
fn alpha_centroid_sort_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
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
    // 近绿（x=0.5）先声明、远蓝（x=0.25）后声明 vs 互换。红墙 x=0 在两
    // 支透明面之后（只读深不剔除墙前混合）。
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
    // ① 开关关：远蓝主导（P0 组内 artifact 的框架级语义编码）。
    let bytes_a = r
        .render_readback(&base(near_first.clone(), false), 64, 64)
        .expect("render off");
    let (ar, ag, ab) = px(&bytes_a);
    assert!(
        ab > 100 && ab > ag,
        "off: later-declared far blue dominates: {ar} {ag} {ab}"
    );
    // ② 开关开（同一声明序）：重排后近绿后画主导。
    let bytes_b = r
        .render_readback(&base(near_first.clone(), true), 64, 64)
        .expect("render on");
    let (br, bg, bb) = px(&bytes_b);
    assert!(
        bg > 100 && bg > bb,
        "on: near green dominates after sort: {br} {bg} {bb}"
    );
    // ③ 声明序互换 + 排序开：输出逐字节一致（最终顶点序相同）。
    let bytes_c = r
        .render_readback(&base(far_first, true), 64, 64)
        .expect("render on swapped");
    assert_eq!(bytes_b, bytes_c, "sorted output independent of declaration order");
}

/// 质心排序纯逻辑（无 GPU，无适配器环境也跑）：距离降序重排、等距
/// stable（保声明序）、开关关/单三角返回 None。「前」视相机眼点在
/// center + (3r, 0, 0)。
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
        // 质心 x = 1 / 3 / 2 → 眼点 (3,0,0)，距离升序 = x 降序 → 远的先画。
        tris_alpha: vec![
            v(1.0, 0.1), v(1.0, 0.1), v(1.0, 0.1), // t0: 最远
            v(3.0, 0.3), v(3.0, 0.3), v(3.0, 0.3), // t1: 最近
            v(2.0, 0.2), v(2.0, 0.2), v(2.0, 0.2), // t2: 中间
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
    // 等距 stable：同 x 全组 → 声明序保持（tag 0.1..0.6 原序）。
    scene.tris_alpha = (0..6).map(|k| v(1.0, 0.1 + k as f32 * 0.1)).collect();
    let sorted = scene.alpha_sorted().unwrap();
    for (k, vtx) in sorted.iter().enumerate() {
        let expect = 0.1 + k as f32 * 0.1;
        assert!((vtx.color[0] - expect).abs() < 1e-6, "stable order at {k}");
    }
    // 开关关 → None；单三角 → None（无可排序的顺序问题）。
    scene.sort_alpha = false;
    assert!(scene.alpha_sorted().is_none());
    scene.sort_alpha = true;
    scene.tris_alpha.truncate(3);
    assert!(scene.alpha_sorted().is_none());
}


/// 透明桶（round-205 P0）：ALPHA_BLENDING + depth write OFF——①墙前透
/// 明面正常混合（0.5 绿 over 红墙 → r≈g）；②墙后透明面被只读深剔除
/// （像素保持墙色）；③同屏两支透明面按声明序混合（后声明者主导重叠
/// 区）。前视（yaw 0 pitch 0）：深度 = 世界 x，越大越近相机。
#[test]
fn transparent_tris_blend_order_and_depth() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
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
    // 红墙覆盖全视口（±3 覆盖 NDC ±1.2）；透明面覆盖视口中心区。
    let wall = |x: f32| vec![rv(x, -3.0, -3.0, RED), rv(x, 3.0, -3.0, RED), rv(x, 0.0, 3.0, RED)];
    let tq = |x: f32, c: [f32; 4]| {
        vec![rv(x, -0.5, -0.5, c), rv(x, 0.5, -0.5, c), rv(x, 0.0, 0.5, c)]
    };
    // BGRA 中心像素 → (r, g, b)。
    let px = |bytes: &[u8]| {
        let i = ((32 * 64 + 32) * 4) as usize;
        (bytes[i + 2], bytes[i + 1], bytes[i])
    };
    // ① 墙 x=0 + 墙前 x=0.5 绿 0.5 → 中心 = 0.5 绿 + 0.5 红 = (127,127,0)。
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
    // ② 墙 x=0 + 墙后 x=-0.5 蓝 0.5 → 只读深剔除，中心保持墙红。
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
    // ③ 声明序：绿 0.5（x=0.5）先、蓝 0.5（x=0.25）后 → 中心 =
    // 0.5 蓝 + 0.5(0.5 绿 + 0.5 红) = (63,63,127)，蓝主导（深度写关，
    // 混合序 = 数组序；两支都在墙前，均通过只读深）。
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

// ---- 体渲染（round-211 P2 MVP，诉求书 §8 断言 ①②③） ----

/// 均匀体 VolumeSlice 构造（dims 8³，LUT 可配）。
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

/// 中心射线积分（诉求书 §8 ①）：均匀体 0.5 加常数不透明度 LUT（8/255）
/// 与 64 步，中心像素解析值 acc_a = 1-(1-8/255)^64 ≈ 0.8696；premult
/// 输出 over 黑底 ≈ LUT 色 × acc_a（±2/255），四角射线不穿 AABB 保持背景。
#[test]
fn volume_center_ray_integration_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
        return;
    };
    // LUT 全档绿色 a=8/255（常数不透明度 MVP）。
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
    // 解析：a = 8/255，acc_a = 1-(1-a)^64，out.g = 255·acc_a（黑底）。
    let a = 8.0f32 / 255.0;
    let acc_a = 1.0 - (1.0 - a).powi(64);
    let expect_g = (255.0 * acc_a).round() as i32;
    let (cr, cg, cb) = px(32, 32);
    assert!(
        (cg as i32 - expect_g).abs() <= 2 && cr <= 2 && cb <= 2,
        "center ray integral: got ({cr},{cg},{cb}) expect (0,{expect_g},0)"
    );
    // 四角射线不穿 AABB → 背景。
    let (br, bg_, bb) = px(2, 2);
    assert!(br < 4 && bg_ < 4 && bb < 4, "corner stays bg: ({br},{bg_},{bb})");
}

/// 撞不透明墙截断（诉求书 §8 ②，归一化步长语义）：steps 均分
/// [t_entry, t_wall]（§4.2）——截断不减少步数而是把采样集中到墙前段。
/// 用「前半 0 / 后半 1」场 + 双色 LUT（0 → 绿、1 → 蓝）：截断时采样全
/// 落前半绿区（b≈0）；不截断则后半蓝区混入（b 显著）。红墙透底
/// （r = (1-acc_a)·255）。
#[test]
fn volume_wall_truncation_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
        return;
    };
    // LUT[0] = 绿 a=8；LUT[255] = 蓝 a=8；中间档透明黑（界面带不产雾）。
    let mut lut = vec![[0u8, 0u8, 0u8, 0u8]; 256];
    lut[0] = [0u8, 255u8, 0u8, 8u8];
    lut[255] = [0u8, 0u8, 255u8, 8u8];
    let rv = |x: f32, y: f32, z: f32| RenderVertex {
        pos: [x, y, z],
        normal: [1.0, 0.0, 0.0],
        color: [1.0, 0.0, 0.0, 1.0],
    };
    // 前视相机（深度轴 = 世界 x）：墙 x=0 横切体 [-0.3,0.3] 中部——
    // 俯视 cam() 下该墙侧对相机（面法线 ⊥ 视线），不触发撞墙。
    let cam = OrthoCamera {
        yaw_deg: 0.0,
        pitch_deg: 0.0,
        center: [0.0, 0.0, 0.0],
        radius: 1.0,
    };
    // 场：格心 x < 0（后半，远侧）值 1.0（蓝）；x ≥ 0（前半，近侧）0.0（绿）。
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
    // 截断：采样全落绿区（前半）→ 透底红 + 绿雾、零蓝。r/g 按
    // acc_a ∈ [全 64 样, 少 7 样]（界面带线性滤波过渡档透明）放宽。
    assert!(
        (30..=60).contains(&cr) && cg > 195 && cb <= 6,
        "wall truncation: got ({cr},{cg},{cb}) expect (30..60,>195,<=6)"
    );
}

/// LUT 端点（诉求书 §8 ③）：值 1.0 均匀体 → 采样 LUT[255]——归一化
/// 贯通的端到端证据（rgb 端点色 × ①同款 acc_a）。
#[test]
fn volume_lut_endpoint_headless() {
    let Some(mut r) = headless_renderer() else {
        eprintln!("skip: 无可用 wgpu 适配器");
        return;
    };
    let mut lut = vec![[0u8, 0u8, 0u8, 8u8]; 256];
    lut[255] = [255u8, 0u8, 0u8, 8u8]; // 端点红（其余黑——采样仅触 1.0）
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
