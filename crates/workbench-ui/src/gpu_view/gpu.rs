//! The wgpu device every GPU view in the process renders on.
//!
//! On the web, GPUI's renderer is wgpu itself, and a viewport can only reach
//! the screen as a texture GPUI samples (`WgpuDynamicTexture`), which has to
//! live on GPUI's own device: so the device is the one the web platform
//! hands out. On the desktop GPUI renders with Direct3D 11 (Windows) or Metal
//! (macOS) — there is no wgpu device to share — so the viewports get a device
//! of their own: DX12 on Windows, where frames go to a swap chain on a
//! DirectComposition visual inside GPUI's window, and the platform's primary
//! backend elsewhere, where frames are read back.

#[cfg(not(target_family = "wasm"))]
use anyhow::{Context as _, Result};

/// Extra device features and limits a product's own renderer needs, beyond
/// the viewport baseline.
///
/// Hand it to `WorkbenchAppBuilder::with_gpu_requirements` (boxed); the
/// frontend applies it before the first GPU view creates the device. On the
/// web the device is GPUI's and already exists, so requirements are checked
/// against it instead and a shortfall is logged.
#[derive(Clone, Debug, Default)]
pub struct GpuRequirements {
    /// Features OR-ed onto the baseline (none).
    pub features: wgpu::Features,
    /// Limits merged with the baseline: upper bounds take the larger value,
    /// alignments and other lower bounds the smaller one.
    pub limits: Option<wgpu::Limits>,
}

impl GpuRequirements {
    /// The limits to request: `base` raised (or tightened) to what the
    /// product asked for.
    #[cfg(not(target_family = "wasm"))]
    fn merged_limits(&self, base: wgpu::Limits) -> wgpu::Limits {
        match &self.limits {
            Some(extra) => base.or_better_values_from(extra),
            None => base,
        }
    }
}

thread_local! {
    static REQUIREMENTS: std::cell::RefCell<GpuRequirements> =
        std::cell::RefCell::new(GpuRequirements::default());
}

/// Record what the product needs from the device. Takes effect only before
/// the device is created (the first GPU view to paint creates it).
pub(crate) fn set_requirements(requirements: GpuRequirements) {
    REQUIREMENTS.with(|r| *r.borrow_mut() = requirements);
}

fn requirements() -> GpuRequirements {
    REQUIREMENTS.with(|r| r.borrow().clone())
}

/// The one wgpu device/queue every GPU view in the process renders on.
///
/// Plugin-provided views obtain it through
/// [`shared_gpu`](crate::gpu_view::shared_gpu) and build their own
/// pipelines on it. Render targets must use [`Gpu::target_format`].
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The color format every viewport frame is rendered in.
    pub target_format: wgpu::TextureFormat,
    /// The device's `max_texture_dimension_2d`.
    pub(crate) max_texture: u32,
    /// Windows: the instance and adapter the device came from, which
    /// composition swap chains are made with.
    #[cfg(target_os = "windows")]
    pub(crate) instance: wgpu::Instance,
    #[cfg(target_os = "windows")]
    pub(crate) adapter: wgpu::Adapter,
    /// Web: GPUI's context, which dynamic textures are allocated from.
    #[cfg(target_family = "wasm")]
    pub(crate) context: gpui_wgpu::WgpuContext,
}

impl Gpu {
    /// Web: GPUI's own device, from the platform's wgpu context.
    #[cfg(target_family = "wasm")]
    pub(crate) fn from_web_context(context: gpui_wgpu::WgpuContext) -> Self {
        let wanted = requirements();
        let missing = wanted.features - context.device.features();
        if !missing.is_empty() {
            log::warn!(
                "gpu viewport: GPUI's web device lacks features the product asked for: {missing:?}"
            );
        }
        Self {
            device: (*context.device).clone(),
            queue: (*context.queue).clone(),
            target_format: context.color_texture_format(),
            max_texture: context.device.limits().max_texture_dimension_2d,
            context,
        }
    }

    /// Desktop: a device of the viewports' own.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn new() -> Result<Self> {
        // Windows: frames go to a DirectComposition visual, which only the
        // DX12 backend can present to.
        #[cfg(target_os = "windows")]
        let backends = wgpu::Backends::DX12;
        #[cfg(not(target_os = "windows"))]
        let backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no suitable GPU adapter found")?;
        let info = adapter.get_info();
        log::info!("gpu viewport: {} ({:?}, driver {})", info.name, info.backend, info.driver);

        let wanted = requirements();
        let limits = wanted.merged_limits(wgpu::Limits::default().using_resolution(adapter.limits()));
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("workbench-gpu-viewport-device"),
            required_features: wanted.features,
            required_limits: limits,
            ..Default::default()
        }))
        .context("failed to open wgpu device")?;
        // Device errors are dispatched asynchronously; wgpu's default
        // handler panics, which would take the UI thread down with it.
        device.on_uncaptured_error(std::sync::Arc::new(|error: wgpu::Error| {
            log::error!("gpu viewport: wgpu error: {error}");
        }));

        Ok(Self {
            max_texture: device.limits().max_texture_dimension_2d,
            device,
            queue,
            // Matches GPUI's own (non-sRGB) BGRA targets, so colors written by
            // the viewport pipelines come out as GPUI would show them; also
            // the readback path's byte order.
            target_format: wgpu::TextureFormat::Bgra8Unorm,
            #[cfg(target_os = "windows")]
            instance,
            #[cfg(target_os = "windows")]
            adapter,
        })
    }
}
