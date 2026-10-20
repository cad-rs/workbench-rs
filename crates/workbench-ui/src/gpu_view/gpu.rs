//! Platform-shared wgpu device context.
//!
//! Every GPU view in the process renders on this one device. On Windows / Web
//! the gpui renderer already owns a wgpu device (DX12 / WebGPU) and a texture
//! created on any other device cannot be composited into its surface, so the
//! handle is taken from the window and kept alive here. On other desktops gpui
//! uses its own renderer, so an independent device is created and frames are
//! rendered offscreen and read back for compositing.
//!
//! Ported from the gpui-wgpu reference implementation (javagg/gpui-demo).

#[cfg(not(any(target_family = "wasm", target_os = "windows")))]
use anyhow::{Context as _, Result};

/// The one wgpu device/queue every GPU view in the process renders on.
///
/// Plugin-provided WebGPU views obtain this through
/// [`super::shared_gpu`](crate::gpu_view::shared_gpu) and build their own
/// pipelines, textures and render targets on it. Sharing the device is not
/// optional on Windows / Web: `paint_surface` composites a texture produced by
/// gpui's own renderer, so the texture must live on that renderer's device.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Surface format every offscreen target uses.
    pub target_format: wgpu::TextureFormat,
    /// Keeps gpui's wgpu context alive (shared-device path).
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    pub handle: gpui_wgpu::WgpuContextHandle,
}

impl Gpu {
    /// The gpui renderer context handle (needed to build a `WgpuRenderTarget`).
    #[cfg(any(target_family = "wasm", target_os = "windows"))]
    pub fn context_handle(&self) -> &gpui_wgpu::WgpuContextHandle {
        &self.handle
    }

    /// Windows: take the shared wgpu context from the gpui window. Returns
    /// `None` before the first frame (renderer not ready yet); callers retry.
    #[cfg(target_os = "windows")]
    pub fn from_window(window: &gpui::Window) -> Option<Self> {
        let handle = gpui_wgpu::WgpuContextHandle::from_window(window)?;
        // DX12 device errors are dispatched asynchronously; without a callback
        // they are silently dropped.
        handle
            .device()
            .on_uncaptured_error(std::sync::Arc::new(|error| {
                eprintln!("wgpu uncaptured error: {error}");
            }));
        Some(Self {
            device: handle.device().clone(),
            queue: handle.queue().clone(),
            target_format: handle.texture_format(),
            handle,
        })
    }

    /// Web: same shared-device path as Windows (gpui's web renderer is WebGPU).
    #[cfg(target_family = "wasm")]
    pub fn from_window(window: &gpui::Window) -> Option<Self> {
        let handle = gpui_wgpu::WgpuContextHandle::from_window(window)?;
        Some(Self {
            device: handle.device().clone(),
            queue: handle.queue().clone(),
            target_format: handle.texture_format(),
            handle,
        })
    }

    /// Other desktops: create a standalone device.
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
