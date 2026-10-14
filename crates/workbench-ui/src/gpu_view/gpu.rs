//! wgpu 设备上下文：所有 GPU 视口共享一个 device/queue。
//! 移植自 gpui-wgpu 参考实现（javagg/gpui-demo）。
//!
//! - Windows / Web：gpui 的渲染器本身就是 wgpu（DX12/WebGPU），必须复用
//!   它的共享设备（跨设备无法合成纹理），通过 `WgpuContextHandle::from_window`
//!   在首帧时惰性获取。
//! - 其它桌面（macOS/Linux）：gpui 用自有渲染器，这里自建独立的 wgpu 设备，
//!   离屏渲染后回读合成。

#[cfg(not(any(target_family = "wasm", target_os = "windows")))]
use anyhow::{Context as _, Result};

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// 目标纹理格式（所有离屏视口统一使用）
    pub target_format: wgpu::TextureFormat,
    /// 保持 GPUI 的 wgpu 上下文存活（Windows / web 共享设备路径）
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    _handle: gpui_wgpu::WgpuContextHandle,
}

impl Gpu {
    /// 共享设备路径：GPUI 渲染器上下文句柄（创建 WgpuRenderTarget 需要）
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    pub fn context_handle(&self) -> &gpui_wgpu::WgpuContextHandle {
        &self._handle
    }

    /// Windows：从 GPUI 窗口拿共享的 wgpu 上下文。首帧之前调用可能返回
    /// `None`（渲染器还没就绪），调用方应当重试。
    #[cfg(target_os = "windows")]
    pub fn from_window(window: &gpui::Window) -> Option<Self> {
        let handle = gpui_wgpu::WgpuContextHandle::from_window(window)?;
        // DX12 的设备错误是异步派发的，不挂回调就完全静默
        handle
            .device()
            .on_uncaptured_error(std::sync::Arc::new(|error| {
                eprintln!("wgpu uncaptured error: {error}");
            }));
        Some(Self {
            device: handle.device().clone(),
            queue: handle.queue().clone(),
            target_format: handle.texture_format(),
            _handle: handle,
        })
    }

    /// Web：同 Windows 一样走共享设备（gpui 的 web 渲染器是 WebGPU/WebGL）。
    #[cfg(target_family = "wasm")]
    pub fn from_window(window: &gpui::Window) -> Option<Self> {
        let handle = gpui_wgpu::WgpuContextHandle::from_window(window)?;
        Some(Self {
            device: handle.device().clone(),
            queue: handle.queue().clone(),
            target_format: handle.texture_format(),
            _handle: handle,
        })
    }

    /// 非 Windows 桌面：自建独立设备。
    #[cfg(not(any(target_family = "wasm", target_os = "windows")))]
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .context("no suitable GPU adapter found")?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("workbench-gpu-viewport-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: Default::default(),
            trace: wgpu::Trace::Off,
        }))
        .context("failed to open wgpu device")?;

        Ok(Self {
            device,
            queue,
            target_format: wgpu::TextureFormat::Bgra8Unorm,
        })
    }
}
