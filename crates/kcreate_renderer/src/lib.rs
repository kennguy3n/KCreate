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

pub mod compute;
pub mod cpu_backend;
pub mod display_list;
pub mod geometry;
pub mod gpu;
pub mod native_surface;
pub mod pipeline;
pub mod presenter;
pub mod readback;
pub mod scene;
pub mod spatial;
pub mod surface;
pub mod text;
pub mod viewport;

use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use thiserror::Error;

pub use cpu_backend::CpuBackend;
pub use display_list::{DisplayCommand, DisplayList};
pub use geometry::{Color, Paint, PathCommand, Point2, Rect, Stroke, Style, Vec2};
pub use gpu::{GpuBackend, GpuTier};
pub use native_surface::NativeSurface;
pub use pipeline::Pipeline;
pub use presenter::{DirtyRect, FrameId, FrameLease, PresentSnapshot, Presenter};
pub use scene::{Object, ObjectId, ObjectKind, Scene};
pub use spatial::SpatialIndex;
pub use viewport::Viewport;

/// Which presentation path the renderer is currently configured to
/// use. Phase 0 is `Offscreen` everywhere; Phase 1 will switch
/// platform-by-platform to `Native` as the Electron child-window
/// embedding lands.
///
/// The pipeline, display list, viewport, and scene-graph layers
/// are presentation-mode-agnostic — only the final "publish a
/// frame" step differs:
///
/// - `Offscreen`: render into [`crate::surface::OffscreenSurface`],
///   read pixels back to a CPU buffer, hand the buffer to the
///   [`Presenter`], publish a [`FrameId`]. The host pulls the
///   buffer over N-API + IPC and paints it on a 2D `<canvas>`.
/// - `Native`: render into the swapchain texture acquired from a
///   [`NativeSurface`], present directly. No readback, no IPC, no
///   `putImageData`.
///
/// This enum carries the active surface (when `Native`) so the
/// publish step can call `present_cpu_frame` on it. It is `Debug`
/// (via the wrapped [`NativeSurface`]) so callers can include it
/// in diagnostic logs.
#[derive(Debug)]
pub enum PresentationMode {
    /// Offscreen rasterization + CPU readback, used by Phase 0
    /// and any host that cannot accept a raw window handle (e.g.
    /// Electron's web view before child-window embedding).
    Offscreen,
    /// Direct swapchain presentation against a real OS window.
    /// The wrapped surface is owned by the renderer for the
    /// duration of the editor session.
    Native(Box<NativeSurface>),
}

impl PresentationMode {
    /// `true` when this mode bypasses the CPU readback path.
    #[must_use]
    pub const fn is_native(&self) -> bool {
        matches!(self, Self::Native(_))
    }
}

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

    /// `true` when the GPU backend is currently installed. The runtime
    /// fallback in [`RenderContext::render_into_staging`] uses this to
    /// decide whether a render error is recoverable by swapping to the
    /// CPU rasterizer (a CPU error is terminal — there is nothing lower
    /// to fall back to).
    const fn is_gpu(&self) -> bool {
        matches!(self, Self::Gpu(_))
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
    /// Test-only fault injection: the number of subsequent GPU renders
    /// to force-fail, so the runtime CPU-fallback path can be exercised
    /// on hosts (including CI under lavapipe) that *do* have a working
    /// software GPU adapter. Compiled out entirely in non-test builds.
    #[cfg(test)]
    forced_gpu_failures: std::sync::atomic::AtomicU32,
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
            #[cfg(test)]
            forced_gpu_failures: std::sync::atomic::AtomicU32::new(0),
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

    /// Rasterize the current frame into `staging` using the active
    /// backend, with a runtime GPU→CPU fallback.
    ///
    /// On a GPU render error (e.g. a `wgpu` device loss or a surface /
    /// readback failure), the GPU backend is swapped for a fresh
    /// software [`CpuBackend`] *in place* and the frame is retried once.
    /// The CPU rasterizer clears and refills `staging`, so reusing the
    /// same buffer after a partially-written GPU attempt is safe. After
    /// the swap [`Self::tier`] reports [`GpuTier::SoftwareFallback`] and
    /// every later frame renders on the CPU.
    ///
    /// Only the backend lock is held here, and it is acquired after the
    /// caller has already released the per-frame `dirty_region`,
    /// `viewport`, and `pipeline` locks — so the renderer's lock
    /// discipline is preserved and the in-place swap cannot deadlock
    /// against a concurrent reader.
    fn render_into_staging(
        &self,
        scene: &Scene,
        viewport: &Viewport,
        display_list: &DisplayList,
        staging: &mut Vec<u8>,
    ) -> Result<()> {
        let size = (self.width, self.height);
        let mut backend = self.backend.lock();

        #[cfg(test)]
        let first = if backend.is_gpu() && self.forced_gpu_failures.load(Ordering::Acquire) > 0 {
            // Test-only fault injection: pretend the GPU render failed so
            // the fallback is exercised on hosts (incl. CI under lavapipe)
            // that do have a working software adapter. Never fabricates a
            // CPU failure.
            self.forced_gpu_failures.fetch_sub(1, Ordering::AcqRel);
            Err(RendererError::Wgpu("injected GPU failure (test)".into()))
        } else {
            backend.render(scene, viewport, display_list, staging, size)
        };
        #[cfg(not(test))]
        let first = backend.render(scene, viewport, display_list, staging, size);

        match first {
            Ok(()) => Ok(()),
            Err(err) if backend.is_gpu() => {
                log::warn!(
                    "kcreate_renderer: GPU render failed ({err}); swapping to CPU \
                     rasterizer and retrying frame"
                );
                *backend = BackendKind::Cpu(CpuBackend::new(self.width, self.height));
                backend.render(scene, viewport, display_list, staging, size)
            }
            Err(err) => Err(err),
        }
    }

    /// Test-only: force the next `n` GPU renders to fail, to exercise the
    /// runtime CPU fallback in [`Self::render_into_staging`] on hosts that
    /// have a working GPU adapter.
    #[cfg(test)]
    fn force_next_gpu_failures(&self, n: u32) {
        self.forced_gpu_failures.store(n, Ordering::Release);
    }

    /// Render the given scene to the offscreen target and publish a new frame.
    ///
    /// Returns the [`FrameId`] of the published frame. If no work was needed
    /// (no dirty region and a frame has previously been published), the
    /// previous frame's id is returned and no GPU/CPU work occurs.
    ///
    /// If the GPU backend errors mid-render (e.g. a `wgpu` device loss),
    /// the renderer swaps it for a software [`CpuBackend`] in place and
    /// retries the same frame once, so a lost adapter degrades to CPU
    /// rasterization instead of freezing the canvas. Init-time
    /// adapter-absence fallback is handled separately in [`init_backend`].
    ///
    /// If the render still errors after that fallback, the dirty region is
    /// restored so a subsequent retry still knows to repaint the affected
    /// area, and the externally-visible frame [`Self::sequence`] is left
    /// unchanged. (The [`FrameId`] allocated for the attempt is simply
    /// skipped — the internal `next_frame_id` allocation counter always
    /// advances, so ids are monotonic but may have gaps after a failure.)
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
        let render_result = self.render_into_staging(scene, &viewport, &display_list, &mut staging);
        match render_result {
            Ok(()) => {
                // `publish_diff` computes the exact changed-pixel
                // rectangle versus the previous frame so the host can
                // present only what moved (see `take_present`).
                self.presenter.publish_diff(frame_id, staging);
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

    /// Render the scene into the swapchain texture of the given
    /// [`NativeSurface`] and present it. This is the Phase 1 fast
    /// path that bypasses the offscreen → readback → IPC chain.
    ///
    /// The GPU backend must be active (Phase 1 swap path uses
    /// `device` + `queue` from `GpuBackend`). On the CPU fallback,
    /// returns `RendererError::Wgpu("native present requires GPU
    /// backend")` because there's no device/queue to upload with.
    ///
    /// Unlike [`Self::render_frame`], the native path does **not**
    /// silently swap to the CPU rasterizer on a GPU loss: a software
    /// backend cannot present into a swapchain. Instead it returns
    /// `Err`, which the caller (`CanvasHost`) treats as the signal to
    /// detach the native surface and fall back to the offscreen / IPC
    /// path — where [`Self::render_frame`] self-heals via the in-place
    /// GPU→CPU swap described above.
    ///
    /// Like [`Self::render_frame`], this is a no-op when nothing is
    /// dirty: it returns the previous [`FrameId`] without re-running
    /// the pipeline or touching the swapchain.
    pub fn render_frame_native(
        &self,
        scene: &Scene,
        native_surface: &NativeSurface,
    ) -> Result<FrameId> {
        let dirty = {
            let mut guard = self.dirty_region.lock();
            guard.take()
        };
        let viewport = *self.viewport.lock();

        if dirty.is_none() && self.sequence.load(Ordering::Acquire) > 0 {
            return Ok(FrameId(self.sequence.load(Ordering::Acquire)));
        }

        let display_list = {
            let mut pipeline = self.pipeline.lock();
            pipeline.build_display_list(scene, &viewport, (self.width, self.height))
        };

        let frame_id = FrameId(self.next_frame_id.fetch_add(1, Ordering::AcqRel));
        let mut staging = self.presenter.acquire_staging(self.width, self.height);

        // Rasterize via the same path the offscreen mode uses — the
        // GPU backend's current rasterizer is a CPU compositor, and
        // the CPU backend obviously is too. Either way we get
        // straight-alpha RGBA8 bytes back, which we then upload
        // straight into the swapchain texture.
        let render_result = self.backend.lock().render(
            scene,
            &viewport,
            &display_list,
            &mut staging,
            (self.width, self.height),
        );

        match render_result {
            Ok(()) => {
                let publish_result = {
                    let backend = self.backend.lock();
                    match &*backend {
                        BackendKind::Gpu(g) => {
                            native_surface.present_cpu_frame(g.device(), g.queue(), &staging)
                        }
                        BackendKind::Cpu(_) => Err(RendererError::Wgpu(
                            "native present requires GPU backend (CPU fallback in use)".into(),
                        )),
                    }
                };
                match publish_result {
                    Ok(()) => {
                        // Keep the presenter's staging recycling path
                        // honest — we never actually published to the
                        // presenter in native mode, so the buffer is
                        // returned to the free list.
                        self.presenter.recycle_staging(staging);
                        self.sequence.store(frame_id.0, Ordering::Release);
                        Ok(frame_id)
                    }
                    Err(e) => {
                        if let Some(rect) = dirty {
                            let mut g = self.dirty_region.lock();
                            *g = Some(g.map_or(rect, |existing| existing.union(&rect)));
                        }
                        self.presenter.recycle_staging(staging);
                        Err(e)
                    }
                }
            }
            Err(e) => {
                if let Some(rect) = dirty {
                    let mut g = self.dirty_region.lock();
                    *g = Some(g.map_or(rect, |existing| existing.union(&rect)));
                }
                self.presenter.recycle_staging(staging);
                Err(e)
            }
        }
    }

    /// Create a [`NativeSurface`] bound to this renderer's GPU device.
    ///
    /// Returns `Err(RendererError::Wgpu(...))` when the renderer is
    /// running on the CPU fallback (the native presentation path
    /// requires `wgpu::Device` / `Queue`, which the
    /// [`crate::cpu_backend::CpuBackend`] does not expose). Callers
    /// detect this and fall back to the offscreen / CPU readback
    /// path (the Block A Task 6 `CanvasHost` toggle does exactly
    /// that).
    ///
    /// The `handle` must outlive the returned `NativeSurface`. The
    /// `'static` bound on `H` makes that contract explicit — callers
    /// typically wrap the OS handle bytes in an
    /// `Arc<PlatformHandle>` that owns its pointer for the duration
    /// of the session.
    pub fn create_native_surface<H>(
        &self,
        handle: std::sync::Arc<H>,
        width: u32,
        height: u32,
    ) -> Result<NativeSurface>
    where
        H: raw_window_handle::HasWindowHandle
            + raw_window_handle::HasDisplayHandle
            + Send
            + Sync
            + 'static,
    {
        let backend = self.backend.lock();
        match &*backend {
            BackendKind::Gpu(g) => NativeSurface::from_window(
                g.instance(),
                g.adapter(),
                g.device(),
                handle,
                width,
                height,
            ),
            BackendKind::Cpu(_) => Err(RendererError::Wgpu(
                "create_native_surface requires GPU backend (CPU fallback in use)".into(),
            )),
        }
    }

    /// Resize an attached [`NativeSurface`]'s swapchain to match a
    /// new (width, height). The renderer's offscreen target is
    /// *separately* resized via [`Self::resize`] — both must be kept
    /// in step when the host canvas changes size, because the
    /// renderer rasterises into the offscreen staging buffer and
    /// then uploads to the swapchain.
    ///
    /// Returns `Err(RendererError::Wgpu(...))` on the CPU fallback
    /// (no `wgpu::Device` to drive the swapchain reconfigure).
    /// Callers that hit this should detach the native surface and
    /// fall back to the offscreen / IPC path.
    pub fn resize_native_surface(
        &self,
        native_surface: &mut NativeSurface,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let backend = self.backend.lock();
        match &*backend {
            BackendKind::Gpu(g) => native_surface.resize(g.device(), width, height),
            BackendKind::Cpu(_) => Err(RendererError::Wgpu(
                "resize_native_surface requires GPU backend (CPU fallback in use)".into(),
            )),
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

    /// Snapshot the latest frame for presentation, returning only the
    /// pixels that changed since the host last consumed a frame.
    ///
    /// This is the dirty-rect present path: on a typical edit the
    /// returned [`PresentSnapshot`] carries just the changed sub-rect
    /// (a few KB) instead of the whole framebuffer (megabytes), and the
    /// accumulated dirty region is reset. Falls back to a full frame on
    /// the first frame, after a resize, or when the change covers more
    /// than `max_partial_fraction` of the framebuffer. Returns `None`
    /// only when nothing has ever been rendered.
    pub fn take_present(&self, max_partial_fraction: f32) -> Option<PresentSnapshot> {
        self.presenter.take_present(max_partial_fraction)
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

    #[test]
    fn presentation_mode_offscreen_reports_not_native() {
        let mode = PresentationMode::Offscreen;
        assert!(!mode.is_native());
    }

    /// `Offscreen` is the Phase 0 default — confirm that the
    /// existing render loop reads as not-native and that the
    /// renderer continues to produce frames via the readback path
    /// when configured this way. We don't construct a `Native`
    /// variant here because the `NativeSurface` constructor
    /// requires a real OS window handle, which headless CI does
    /// not have.
    #[test]
    fn offscreen_mode_continues_to_publish_frames() {
        let mode = PresentationMode::Offscreen;
        assert!(matches!(mode, PresentationMode::Offscreen));
        let ctx = initialize(16, 16).expect("init");
        let scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let frame = ctx.render_frame(&scene).expect("render");
        assert!(ctx.get_frame_pixels(frame).is_some());
    }

    /// A GPU device loss *during* a session must not freeze the canvas:
    /// the renderer should swap to the software rasterizer in place,
    /// retry the frame, and keep publishing. (Init-time adapter absence
    /// is covered by `init_backend`; this exercises the runtime path.)
    ///
    /// The injected failure fires *before* `backend.render` is called, so
    /// the test never performs a real GPU submit/readback — that keeps it
    /// deterministic under headless software Vulkan, where the readback
    /// path is flaky — while still driving the in-place GPU→CPU swap on
    /// any host that brought up a GPU adapter.
    #[test]
    fn runtime_gpu_failure_falls_back_to_cpu_and_keeps_presenting() {
        let ctx = initialize(64, 64).expect("init");
        let scene = Scene::new(Color::rgba(0.1, 0.2, 0.3, 1.0));

        // Simulate a wgpu device loss on the next render. On a host that
        // brought up a GPU adapter this drives the runtime GPU→CPU swap;
        // on a host already on the software rasterizer (no adapter /
        // `cpu-only`) `is_gpu()` is false so the injection is a no-op and
        // the test still asserts rendering keeps working.
        ctx.force_next_gpu_failures(1);
        let first = ctx
            .render_frame(&scene)
            .expect("frame still publishes after a GPU loss");

        // The canvas kept presenting on the software rasterizer (after the
        // swap, or because we were already there)...
        assert_eq!(ctx.tier(), GpuTier::SoftwareFallback);
        // ...and the published frame is a complete, valid buffer. Read the
        // length out immediately so the `FrameLease` (a presenter read
        // guard) is dropped before the next `render_frame` publishes —
        // the presenter lock is not reentrant.
        let pixel_len = ctx
            .get_frame_pixels(first)
            .expect("frame after fallback")
            .pixels()
            .len();
        assert_eq!(pixel_len, 64 * 64 * 4);

        // The swap is sticky: subsequent frames keep rendering on the CPU
        // with no further injection, and the frame id keeps advancing.
        ctx.invalidate_all();
        let second = ctx.render_frame(&scene).expect("subsequent frame");
        assert!(
            second.0 > first.0,
            "frame id should advance after fallback: {second:?} !> {first:?}"
        );
        assert_eq!(ctx.tier(), GpuTier::SoftwareFallback);
    }
}
