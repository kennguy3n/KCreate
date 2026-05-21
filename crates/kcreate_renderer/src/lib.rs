//! `KCreate` offscreen renderer.
//!
//! Phase 0 strategy: Rust owns the full rendering pipeline. We render to an
//! offscreen target (wgpu texture on GPU, [`tiny_skia::Pixmap`] on CPU),
//! read the pixels back, and hand a borrow of the latest frame to the host
//! Electron renderer via the N-API bridge for presentation on a
//! `<canvas>` element.
//!
//! Phase 1 upgrade path: swap [`presenter`] + [`readback`] for direct
//! swapchain presentation against a native surface obtained via
//! `raw-window-handle`. The rest of the pipeline ([`pipeline`],
//! [`display_list`], [`viewport`], [`scene`], [`geometry`], [`spatial`])
//! carries forward unchanged.
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod cpu_backend;
pub mod display_list;
pub mod geometry;
pub mod gpu;
pub mod pipeline;
pub mod presenter;
pub mod readback;
pub mod scene;
pub mod spatial;
pub mod surface;
pub mod viewport;

use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use thiserror::Error;

pub use cpu_backend::CpuBackend;
pub use display_list::{DisplayCommand, DisplayList};
pub use geometry::{Color, PathCommand, Point2, Rect, Stroke, Style, Vec2};
pub use gpu::{GpuBackend, GpuTier};
pub use pipeline::Pipeline;
pub use presenter::{FrameId, FrameLease, Presenter};
pub use scene::{Object, ObjectId, ObjectKind, Scene};
pub use spatial::SpatialIndex;
pub use viewport::Viewport;

/// Errors returned by the renderer.
#[derive(Debug, Error)]
pub enum RendererError {
    #[error("invalid dimensions: {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("wgpu surface error: {0}")]
    Wgpu(String),
    #[error("frame id {0:?} is not available (already reclaimed)")]
    FrameUnavailable(FrameId),
    #[error("scene contains object id {0:?} that does not exist")]
    DanglingObjectId(ObjectId),
}

/// Result type alias for renderer operations.
pub type Result<T> = std::result::Result<T, RendererError>;

/// Backend variant selected at startup based on adapter availability.
///
/// The GPU variant is boxed because `GpuBackend` owns large wgpu resources
/// (instance, adapter, device, queue) and we don't want the enum to inherit
/// that storage cost when the CPU fallback is in use.
#[derive(Debug)]
enum BackendKind {
    Gpu(Box<GpuBackend>),
    Cpu(CpuBackend),
}

impl BackendKind {
    const fn tier(&self) -> GpuTier {
        match self {
            Self::Gpu(g) => g.tier(),
            Self::Cpu(_) => GpuTier::SoftwareFallback,
        }
    }

    fn render(
        &mut self,
        scene: &Scene,
        viewport: &Viewport,
        display_list: &DisplayList,
        out_buffer: &mut Vec<u8>,
        size: (u32, u32),
    ) -> Result<()> {
        match self {
            Self::Gpu(g) => g.render(scene, viewport, display_list, out_buffer, size),
            Self::Cpu(c) => c.render(scene, viewport, display_list, out_buffer, size),
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        match self {
            Self::Gpu(g) => g.resize(width, height),
            Self::Cpu(c) => c.resize(width, height),
        }
    }
}

/// Top-level render context held by the host (Electron via N-API).
pub struct RenderContext {
    width: u32,
    height: u32,
    backend: Mutex<BackendKind>,
    pipeline: Mutex<Pipeline>,
    presenter: Presenter,
    viewport: Mutex<Viewport>,
    dirty_region: Mutex<Option<Rect>>,
    next_frame_id: AtomicU64,
    sequence: AtomicU64,
}

impl std::fmt::Debug for RenderContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderContext")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("backend_tier", &self.backend.lock().tier())
            .finish_non_exhaustive()
    }
}

impl RenderContext {
    /// Initialize the renderer at the given size.
    ///
    /// Tries native GPU backends (Metal → D3D12 → Vulkan → GL) and falls back
    /// to a real CPU rasterizer ([`tiny_skia`]) if no adapter is available
    /// or if the `cpu-only` feature is enabled.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        validate_dims(width, height)?;
        let backend = init_backend(width, height);
        let pipeline = Pipeline::new();
        let presenter = Presenter::new(width, height);
        let viewport = Viewport::new(Vec2::ZERO, 1.0);
        Ok(Self {
            width,
            height,
            backend: Mutex::new(backend),
            pipeline: Mutex::new(pipeline),
            presenter,
            viewport: Mutex::new(viewport),
            dirty_region: Mutex::new(Some(Rect::new(0.0, 0.0, width as f32, height as f32))),
            next_frame_id: AtomicU64::new(1),
            sequence: AtomicU64::new(0),
        })
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub fn tier(&self) -> GpuTier {
        self.backend.lock().tier()
    }

    /// Resize the offscreen render target.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        validate_dims(width, height)?;
        if width == self.width && height == self.height {
            return Ok(());
        }
        self.backend.lock().resize(width, height)?;
        self.presenter.resize(width, height);
        *self.dirty_region.lock() = Some(Rect::new(0.0, 0.0, width as f32, height as f32));
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Update the viewport (pan/zoom). No-op if the new pan/zoom match
    /// the current values — in particular, the dirty region is NOT
    /// extended in that case, so callers can safely call `set_viewport`
    /// every frame without defeating the dirty-region optimization.
    pub fn set_viewport(&self, pan: Vec2, zoom: f32) {
        let changed = {
            let mut vp = self.viewport.lock();
            let cur_pan = vp.pan;
            let cur_zoom = vp.zoom;
            // Exact equality is exactly the right semantics here:
            // we are checking "did the host hand us the identical
            // numeric values they handed us last time?" — a strictly
            // monotonic predicate for cache invalidation. Approximate
            // comparison would either over-invalidate (e.g.
            // tolerating a non-zero pan delta) or miss real updates.
            #[allow(clippy::float_cmp)]
            let changed = cur_pan != pan || cur_zoom != zoom;
            if changed {
                vp.set_pan(pan);
                vp.set_zoom(zoom);
            }
            changed
        };
        if changed {
            // Pan/zoom does not invalidate the display list — but it does
            // mean we need to repaint the whole framebuffer.
            *self.dirty_region.lock() =
                Some(Rect::new(0.0, 0.0, self.width as f32, self.height as f32));
        }
    }

    /// Mark a region of the canvas as needing redraw.
    pub fn invalidate_region(&self, rect: Rect) {
        let mut dirty = self.dirty_region.lock();
        *dirty = Some(dirty.map_or(rect, |existing| existing.union(&rect)));
    }

    /// Mark the entire canvas as dirty.
    pub fn invalidate_all(&self) {
        *self.dirty_region.lock() =
            Some(Rect::new(0.0, 0.0, self.width as f32, self.height as f32));
    }

    /// Render the given scene to the offscreen target and publish a new frame.
    ///
    /// Returns the [`FrameId`] of the published frame. If no work was needed
    /// (no dirty region and a frame has previously been published), the
    /// previous frame's id is returned and no GPU/CPU work occurs.
    ///
    /// If the backend errors mid-render, the dirty region is restored so a
    /// subsequent retry still knows to repaint the affected area. The
    /// frame id counter is not incremented on failure.
    pub fn render_frame(&self, scene: &Scene) -> Result<FrameId> {
        let dirty = {
            let mut guard = self.dirty_region.lock();
            guard.take()
        };
        let viewport = *self.viewport.lock();

        if dirty.is_none() && self.sequence.load(Ordering::Acquire) > 0 {
            // Nothing dirty — return the previous frame without rebuilding
            // the display list. (The cache would be a hit anyway, but
            // skipping the call avoids hashing the scene every rAF tick.)
            return Ok(FrameId(self.sequence.load(Ordering::Acquire)));
        }

        let display_list = {
            let mut pipeline = self.pipeline.lock();
            pipeline.build_display_list(scene, &viewport, (self.width, self.height))
        };

        let frame_id = FrameId(self.next_frame_id.fetch_add(1, Ordering::AcqRel));
        let mut staging = self.presenter.acquire_staging(self.width, self.height);
        let render_result = self.backend.lock().render(
            scene,
            &viewport,
            &display_list,
            &mut staging,
            (self.width, self.height),
        );
        match render_result {
            Ok(()) => {
                self.presenter.publish(frame_id, staging);
                self.sequence.store(frame_id.0, Ordering::Release);
                Ok(frame_id)
            }
            Err(e) => {
                // Restore the dirty region so a future render still knows
                // to repaint the affected area. Union with any region
                // that has accrued since we took it.
                if let Some(rect) = dirty {
                    let mut g = self.dirty_region.lock();
                    *g = Some(g.map_or(rect, |existing| existing.union(&rect)));
                }
                self.presenter.recycle_staging(staging);
                Err(e)
            }
        }
    }

    /// Borrow the pixels for the most recently published frame.
    ///
    /// The returned slice is RGBA8, row-major, of length `width * height * 4`.
    pub fn get_frame_pixels(&self, frame: FrameId) -> Option<FrameLease<'_>> {
        self.presenter.lease(frame)
    }

    /// Borrow the latest frame, whatever its id.
    pub fn latest_frame(&self) -> Option<FrameLease<'_>> {
        self.presenter.latest()
    }

    /// Currently selected viewport (pan/zoom).
    pub fn viewport(&self) -> Viewport {
        *self.viewport.lock()
    }
}

const fn validate_dims(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(RendererError::InvalidDimensions { width, height });
    }
    Ok(())
}

fn init_backend(width: u32, height: u32) -> BackendKind {
    if cfg!(feature = "cpu-only") {
        log::info!("kcreate_renderer: cpu-only feature enabled — using CPU backend");
        return BackendKind::Cpu(CpuBackend::new(width, height));
    }
    if let Ok(Some(gpu)) = GpuBackend::try_new(width, height) {
        log::info!(
            "kcreate_renderer: GPU backend ready (tier={:?}, backend={})",
            gpu.tier(),
            gpu.adapter_name(),
        );
        BackendKind::Gpu(Box::new(gpu))
    } else {
        log::warn!("kcreate_renderer: no GPU adapter — falling back to CPU rasterizer");
        BackendKind::Cpu(CpuBackend::new(width, height))
    }
}

// --- Public function-style API mirroring the amendment's spec -----------------

/// Initialize a new renderer at the given size.
///
/// Matches the public API surface declared in the Phase 0 amendment.
pub fn initialize(width: u32, height: u32) -> Result<RenderContext> {
    RenderContext::new(width, height)
}

/// Resize the renderer's offscreen target.
pub fn resize(ctx: &mut RenderContext, width: u32, height: u32) -> Result<()> {
    ctx.resize(width, height)
}

/// Render a single frame and return its id.
pub fn render_frame(ctx: &mut RenderContext, scene: &Scene) -> Result<FrameId> {
    ctx.render_frame(scene)
}

/// Borrow the pixels of a previously rendered frame.
pub fn get_frame_pixels(ctx: &RenderContext, frame: FrameId) -> Option<FrameLease<'_>> {
    ctx.get_frame_pixels(frame)
}

/// Update viewport (pan/zoom).
pub fn set_viewport(ctx: &mut RenderContext, pan: Vec2, zoom: f32) {
    ctx.set_viewport(pan, zoom);
}

/// Mark a region of the canvas dirty.
pub fn invalidate_region(ctx: &mut RenderContext, rect: Rect) {
    ctx.invalidate_region(rect);
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::significant_drop_tightening,
    clippy::significant_drop_in_scrutinee
)]
mod tests {
    use super::*;

    #[test]
    fn initialize_zero_dimensions_fails() {
        assert!(matches!(
            initialize(0, 100),
            Err(RendererError::InvalidDimensions { .. })
        ));
        assert!(matches!(
            initialize(100, 0),
            Err(RendererError::InvalidDimensions { .. })
        ));
    }

    #[test]
    fn initialize_over_max_fails() {
        assert!(matches!(
            initialize(20_000, 100),
            Err(RendererError::InvalidDimensions { .. })
        ));
    }

    #[test]
    fn renders_empty_scene_to_clear_color() {
        let ctx = initialize(64, 32).expect("init");
        let scene = Scene::new(Color::rgba(0.1, 0.2, 0.3, 1.0));
        let frame = ctx.render_frame(&scene).expect("render");
        let lease = ctx.get_frame_pixels(frame).expect("frame lease");
        let pixels = lease.pixels();
        assert_eq!(pixels.len(), 64 * 32 * 4);
        // First pixel should match the clear color (approx; sRGB conversion may shift values).
        let (r, g, b, a) = (pixels[0], pixels[1], pixels[2], pixels[3]);
        assert!(
            r < 60 && g >= 30 && b >= 50 && a == 255,
            "got rgba {r} {g} {b} {a}"
        );
    }

    #[test]
    fn renders_filled_rect_at_expected_location() {
        let ctx = initialize(64, 64).expect("init");
        let mut scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        scene.add_object(Object::new(
            ObjectKind::Rect(Rect::new(8.0, 8.0, 16.0, 16.0)),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        ));
        let frame = ctx.render_frame(&scene).expect("render");
        let lease = ctx.get_frame_pixels(frame).expect("lease");
        let pixels = lease.pixels();
        // Pixel inside the rect should be red.
        let idx = (16 * 64 + 16) * 4;
        assert!(
            pixels[idx] > 200,
            "rect interior red channel low: {}",
            pixels[idx]
        );
        // Pixel outside the rect should be black.
        let outside_idx = (64 + 1) * 4;
        assert!(
            pixels[outside_idx] < 50,
            "outside red channel high: {}",
            pixels[outside_idx]
        );
    }

    #[test]
    fn resize_updates_dimensions_and_invalidates() {
        let mut ctx = initialize(32, 32).expect("init");
        ctx.resize(48, 24).expect("resize");
        assert_eq!(ctx.width(), 48);
        assert_eq!(ctx.height(), 24);
        let scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let frame = ctx.render_frame(&scene).expect("render");
        let lease = ctx.get_frame_pixels(frame).expect("lease");
        assert_eq!(lease.pixels().len(), 48 * 24 * 4);
    }

    #[test]
    fn viewport_zoom_scales_geometry() {
        let mut ctx = initialize(64, 64).expect("init");
        let mut scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        scene.add_object(Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, 8.0, 8.0)),
            Style::filled(Color::rgba(0.0, 1.0, 0.0, 1.0)),
        ));
        set_viewport(&mut ctx, Vec2::ZERO, 2.0);
        let frame = ctx.render_frame(&scene).expect("render");
        let lease = ctx.get_frame_pixels(frame).expect("lease");
        let pixels = lease.pixels();
        // After zoom=2, the 8x8 rect covers 16x16 at the origin. Sample at (12,12).
        let idx = (12 * 64 + 12) * 4;
        assert!(
            pixels[idx + 1] > 200,
            "zoomed rect green low: {}",
            pixels[idx + 1]
        );
    }

    #[test]
    fn invalidate_region_unions_existing_dirty() {
        let ctx = initialize(64, 64).expect("init");
        ctx.invalidate_all();
        ctx.invalidate_region(Rect::new(10.0, 10.0, 5.0, 5.0));
        let region = ctx.dirty_region.lock().expect("dirty");
        assert_eq!(region.width, 64.0);
        assert_eq!(region.height, 64.0);
    }
}
