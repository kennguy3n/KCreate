//! Renderer state machine, decoupled from N-API.
//!
//! Everything here is plain Rust — testable with `cargo test` and reusable
//! by non-N-API consumers (e.g. headless tooling). The N-API wrappers in
//! `lib.rs` are thin: argument marshalling, status conversion, that's it.

use std::sync::OnceLock;

use kcreate_renderer::{
    initialize as renderer_initialize, FrameId, Rect, RenderContext, Scene, Vec2,
};
use parking_lot::Mutex;
use thiserror::Error;

use crate::wire::{parse_scene, WireError};

/// Errors from the bridge layer.
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("renderer not initialized — call renderer_init first")]
    NotInitialized,
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Renderer(#[from] kcreate_renderer::RendererError),
    /// Shared-memory present path failure (mmap, bad descriptor,
    /// geometry mismatch). Only reachable in `native_canvas` builds.
    #[cfg(feature = "native_canvas")]
    #[error(transparent)]
    SharedPresent(#[from] crate::shared_present::SharedPresentError),
}

pub type Result<T> = std::result::Result<T, BridgeError>;

/// Process-wide renderer state. One renderer per process.
fn slot() -> &'static Mutex<Option<RenderContext>> {
    static RENDERER: OnceLock<Mutex<Option<RenderContext>>> = OnceLock::new();
    RENDERER.get_or_init(|| Mutex::new(None))
}

/// Latest scene handed to `render`. Cached so PNG export can re-render
/// at arbitrary scales without round-tripping JSON through JS.
fn scene_slot() -> &'static Mutex<Option<Scene>> {
    static SCENE: OnceLock<Mutex<Option<Scene>>> = OnceLock::new();
    SCENE.get_or_init(|| Mutex::new(None))
}

/// Initialize the renderer at the given size.
///
/// Idempotent: if a renderer already exists at the requested dimensions,
/// returns the existing renderer's info. If it exists at different
/// dimensions, the existing renderer is resized in place (the GPU device
/// is preserved — only the offscreen surface and presenter buffers are
/// reallocated). This means React `<CanvasHost>` mount/unmount cycles
/// and width/height prop changes do not tear down the GPU device.
pub fn init(width: u32, height: u32) -> Result<RendererInfo> {
    let mut guard = slot().lock();
    if let Some(ctx) = guard.as_mut() {
        if ctx.width() != width || ctx.height() != height {
            ctx.resize(width, height)?;
        }
        let info = RendererInfo {
            tier: format!("{:?}", ctx.tier()),
            width: ctx.width(),
            height: ctx.height(),
        };
        drop(guard);
        return Ok(info);
    }
    let ctx = renderer_initialize(width, height)?;
    let info = RendererInfo {
        tier: format!("{:?}", ctx.tier()),
        width: ctx.width(),
        height: ctx.height(),
    };
    *guard = Some(ctx);
    drop(guard);
    Ok(info)
}

/// Shut down the renderer (no-op if not initialized).
///
/// Each `*slot().lock() = None;` statement is an acquire-then-drop:
/// the lock guard's lifetime ends at the semicolon, so we never hold
/// two of `{native_slot, slot, scene_slot}` at the same time here.
/// The render path (which DOES co-hold multiple guards) acquires in
/// the strict order `slot -> native_slot -> scene_slot`, and because
/// shutdown holds zero of those simultaneously, the orders can't
/// invert. Don't refactor this into a helper that takes all three
/// guards at once without re-reading the deadlock analysis on the
/// `Comment 54` thread of PR #5.
pub fn shutdown() {
    #[cfg(feature = "native_canvas")]
    {
        *native_slot().lock() = None;
        // Tear down the shared-memory present ring (publisher unlinks
        // its tmpfs file on drop) and any local reader. Acquire-then-drop,
        // co-holding nothing else, so the canonical lock order is safe.
        *shared_present_slot().lock() = None;
        *shared_reader_slot().lock() = None;
    }
    *slot().lock() = None;
    *scene_slot().lock() = None;
}

/// Test-only helper: reset state so each test starts clean. Not exposed
/// via N-API.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    #[cfg(feature = "native_canvas")]
    {
        *native_slot().lock() = None;
        *shared_present_slot().lock() = None;
        *shared_reader_slot().lock() = None;
    }
    *slot().lock() = None;
    *scene_slot().lock() = None;
}

/// Resize both the offscreen pipeline *and* (if attached) the native
/// presentation surface in a single call.
///
/// The two outputs must stay in step: the renderer rasterises into
/// the offscreen staging buffer at `(width, height)` and then either
/// publishes via the presenter (offscreen mode) or uploads into the
/// swapchain (native mode). If only the offscreen target were
/// resized, native-mode frames would either get clipped (swapchain
/// smaller than staging) or letterboxed with stale pixels
/// (swapchain larger than staging). The host calls this once on
/// every `<canvas>` size change and we fan it out internally.
///
/// On the CPU fallback the offscreen pipeline still resizes
/// normally; a native surface cannot be attached on CPU-only
/// renderers (see [`switch_native`]), so the native branch is
/// a no-op in that case.
///
/// **Partial-failure recovery.** Devin Review PR #5 ANALYSIS-0001
/// (commit 4ee9970) flagged a state-machine hole: if the offscreen
/// resize succeeds but the native swapchain reconfigure fails (e.g.
/// device loss, OOM on a constrained Wayland session), the
/// offscreen pipeline is already committed to the new size while
/// the native swapchain still has the old configuration. The next
/// `render_scene` would rasterise into a `(width, height)` staging
/// buffer but upload to a stale-sized swapchain, producing garbled
/// output or a wgpu validation error. We recover by dropping the
/// native surface entirely on resize failure: the next frame falls
/// back to the offscreen path (which is already correctly resized),
/// and the renderer surfaces a `NativeResizeFailed` error so the
/// host can clear its `requestedMode` toggle and emit a fallback
/// reason via `onNativeFallback`. Re-attaching the native surface
/// later (`switch_native`) is a clean rebuild from the platform
/// handle and won't inherit the broken swapchain.
pub fn resize(width: u32, height: u32) -> Result<()> {
    let mut guard = slot().lock();
    let ctx = guard.as_mut().ok_or(BridgeError::NotInitialized)?;
    ctx.resize(width, height)?;
    // Keep an attached native swapchain in step. The `native_canvas`
    // feature must be enabled for `native_slot` to exist; default
    // builds skip this branch entirely.
    #[cfg(feature = "native_canvas")]
    {
        // The shared-memory ring is sized to the old framebuffer
        // geometry, so a resize invalidates its descriptor. Drop the
        // publisher (its tmpfs file is unlinked on drop); the host
        // re-handshakes via `shared_present_enable` at the new size and
        // `CanvasHost` re-opens the reader. Until then the present path
        // falls back to dirty-rect IPC — no stale-geometry frames.
        *shared_present_slot().lock() = None;
        let mut native = native_slot().lock();
        if let Some(surface) = native.as_mut() {
            match ctx.resize_native_surface(surface, width, height) {
                Ok(()) => {}
                Err(e) => {
                    // Drop the broken native surface so subsequent
                    // `render_scene` calls take the offscreen path,
                    // which is already correctly sized at
                    // (width, height). Holding on to a stale-sized
                    // swapchain would corrupt every subsequent
                    // native-mode frame.
                    *native = None;
                    drop(native);
                    drop(guard);
                    return Err(e.into());
                }
            }
        }
    }
    drop(guard);
    Ok(())
}

pub fn set_viewport(pan_x: f32, pan_y: f32, zoom: f32) -> Result<()> {
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    ctx.set_viewport(Vec2::new(pan_x, pan_y), zoom);
    drop(guard);
    Ok(())
}

/// Read the current viewport pixels-per-scene-unit zoom factor.
///
/// Used by `document::sync_scene_locked` to size remote-peer
/// cursors in screen space (`append_presence_cursors` divides
/// screen-pixel constants by this value so the on-screen cursor
/// triangle is a constant size regardless of pan/zoom).
///
/// Returns `Ok(1.0)` when no renderer is attached so the caller can
/// fall back to world-space sizing in headless contexts without a
/// special-case branch.
pub fn viewport_zoom() -> f32 {
    let guard = slot().lock();
    match guard.as_ref() {
        Some(ctx) => ctx.viewport().zoom,
        None => 1.0,
    }
}

pub fn invalidate(region: Option<Rect>) -> Result<()> {
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    if let Some(r) = region {
        ctx.invalidate_region(r);
    } else {
        ctx.invalidate_all();
    }
    drop(guard);
    Ok(())
}

pub fn render(scene_json: &str) -> Result<FrameId> {
    let scene = parse_scene(scene_json)?;
    render_scene(scene)
}

/// Render an already-parsed [`Scene`].
///
/// This bypasses the JSON wire format and is used by
/// [`crate::document`] when it synchronises the document graph
/// directly into the renderer — JSON round-tripping a scene built
/// in-process is wasted work, and the wire format can't represent
/// every native scene (e.g. the [`SceneSync`] selection highlights
/// reuse `ObjectKind::Rect` with a stroked-only style that the wire
/// `WireKind::Rect` shape also covers, but skipping serialisation
/// keeps the hot path tight regardless).
pub fn render_scene(scene: Scene) -> Result<FrameId> {
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    // Route to the native-surface fast path when one is attached.
    // Default builds don't compile this branch (the `native_canvas`
    // feature is off), so the binary identical to Phase 0 falls
    // through to the offscreen path.
    //
    // **Lock acquisition order: `slot → native_slot → scene_slot`.**
    // This branch holds three mutex guards simultaneously (the
    // single most complex synchronisation point in the bridge). The
    // ordering is consistent with `resize`, `switch_native`, and
    // `switch_offscreen`, and `shutdown` deliberately drops each
    // guard before acquiring the next so it can never invert the
    // order. Devin Review PR #5 ANALYSIS-0005 (commit 4ee9970)
    // confirmed no deadlock is reachable; the authoritative analysis
    // lives on the `shutdown()` doc comment.
    #[cfg(feature = "native_canvas")]
    {
        let native = native_slot().lock();
        if let Some(surface) = native.as_ref() {
            let id = ctx.render_frame_native(&scene, surface)?;
            *scene_slot().lock() = Some(scene);
            drop(native);
            drop(guard);
            return Ok(id);
        }
    }
    let id = ctx.render_frame(&scene)?;
    // Publish the scene snapshot for PNG export *before* releasing the
    // renderer lock. The render lock is the single serialisation point
    // for the renderer; doing the `scene_slot` write inside it makes
    // (frame, scene) advance atomically with respect to any concurrent
    // observer. ANALYSIS_0006 on PR #2 noted that the original
    // drop-then-publish ordering was benign because N-API runs on the
    // JS event loop (no concurrent `render` calls in practice), but
    // moving the write inside the lock is defense-in-depth for the
    // day a future worker thread, off-main-thread napi async task,
    // or test harness drives `render` concurrently — and it costs us
    // exactly nothing because the renderer lock is already held.
    //
    // `scene_slot()` is a separate mutex from `slot()`, so this can't
    // deadlock; the lock order is `slot -> scene_slot` and never the
    // reverse (`current_scene()` only takes `scene_slot`).
    *scene_slot().lock() = Some(scene);
    // Mirror the freshly published full frame into the shared-memory
    // ring when a publisher is attached (offscreen path only — the
    // native branch returned above). No-op in default builds and when
    // shared present is disabled, so the dirty-rect IPC path is
    // untouched. The `slot` guard is still held; `scene_slot` was
    // released at the statement above, so we acquire only the innermost
    // `shared_present_slot` here.
    #[cfg(feature = "native_canvas")]
    publish_shared_locked(ctx, id);
    drop(guard);
    Ok(id)
}

/// Snapshot of the most recently rendered scene. Used by PNG export
/// to drive a fresh offscreen render at the caller's chosen size.
pub fn current_scene() -> Result<Scene> {
    scene_slot()
        .lock()
        .clone()
        .ok_or(BridgeError::NotInitialized)
}

/// Number of objects in the most recently rendered scene, or `0` when
/// no scene has been published yet.
///
/// This counts the renderer's actual draw objects — what is on screen —
/// which is what the perf HUD's "Nodes" row reports. It is intentionally
/// decoupled from the document graph's node count: a directly-seeded
/// scene (the dev-only dense-doc affordance) has no backing document, so
/// the document node count would read zero while thousands of objects are
/// being rendered and panned/zoomed.
pub fn scene_object_count() -> usize {
    scene_slot()
        .lock()
        .as_ref()
        .map_or(0, |scene| scene.objects.len())
}

/// Dev-only: build a dense "analytics dashboard" scene of ~`target_nodes`
/// objects sized to `width`×`height` and render it directly into the live
/// renderer, returning `(frame_id, object_count)`.
///
/// This bypasses the document graph on purpose. It exists so the perf HUD
/// can be captured under real pan/zoom churn on a 5k–10k node document
/// without plumbing a synthetic document through the whole editing path.
/// The seeded scene is cached in `scene_slot` like any other, so
/// [`render_current`] / [`set_viewport_and_render`] re-render it on every
/// pan/zoom tick; the next document mutation (scene sync) replaces it.
///
/// Compiled only behind the `dev_seed` feature — production builds never
/// carry the dense-doc generator. The N-API wrapper returns a
/// "feature not compiled in" error when the feature is off, keeping the
/// IPC surface stable.
#[cfg(feature = "dev_seed")]
pub fn seed_dense_scene(target_nodes: u32, width: f32, height: f32) -> Result<(FrameId, usize)> {
    let doc =
        kcreate_renderer::dense_doc::build_dense_document(target_nodes as usize, width, height);
    let object_count = doc.scene.objects.len();
    let id = render_scene(doc.scene)?;
    Ok((id, object_count))
}

/// Re-render the most recently published scene at the renderer's
/// current viewport and size, returning the new [`FrameId`] — or
/// `Ok(None)` when no scene has been published yet (nothing to
/// repaint) or no renderer is attached (headless / pre-init).
///
/// This is the explicit "repaint what's already on screen" entry
/// point used by the present surface after a viewport (pan/zoom) or
/// resize change. Those operations mark the renderer dirty but do not
/// by themselves rebuild a frame, so the host calls this to produce a
/// fresh frame from the last document-synced scene without shipping
/// the whole scene back across the IPC boundary every frame.
///
/// Unlike [`render_scene`], this borrows the cached scene **in place**
/// under `scene_slot` and renders it directly — it never clones the
/// scene out and never writes an identical copy back. For large
/// documents (thousands of objects) the previous clone-then-write-back
/// was wasted allocation on the pan/zoom hot path (Devin Review #44).
///
/// Lock order is the canonical `slot -> native_slot -> scene_slot`,
/// matching [`render_scene`] and [`shutdown`]: we take the renderer
/// lock first, then (native builds only) the native-surface lock, and
/// finally `scene_slot` for the borrow — so the order can never invert.
pub fn render_current() -> Result<Option<FrameId>> {
    let guard = slot().lock();
    let Some(ctx) = guard.as_ref() else {
        // No renderer attached (headless / pre-init / post-shutdown):
        // there is nothing on screen to repaint. Preserves the prior
        // contract where an empty `scene_slot` yielded `Ok(None)`.
        return Ok(None);
    };
    let id = render_cached_scene_locked(ctx)?;
    drop(guard);
    Ok(id)
}

/// Set the viewport pan/zoom **and** repaint the cached scene in one
/// locked operation, returning the new [`FrameId`] — or `Ok(None)`
/// when no scene has been published yet (nothing to repaint) or no
/// renderer is attached (headless / pre-init / post-shutdown).
///
/// This is the present surface's pan/zoom hot path. Folding the
/// viewport write and the repaint into a single entry point collapses
/// what were two separate N-API/IPC round-trips ([`set_viewport`] then
/// [`render_current`]) into one, halving the bridge crossings while the
/// user is actively panning or zooming. The viewport write marks the
/// renderer dirty only when the pan/zoom actually changes, so the
/// subsequent repaint publishes a fresh frame for a real interaction
/// and reuses the cached frame id for a no-op.
///
/// Lock order is the canonical `slot -> native_slot -> scene_slot`:
/// the viewport write and the repaint share the single `slot` guard
/// acquired here, and the repaint borrows the cached scene in place
/// via [`render_cached_scene_locked`] — matching [`render_scene`],
/// [`render_current`], and [`shutdown`], so the order can never invert.
pub fn set_viewport_and_render(pan_x: f32, pan_y: f32, zoom: f32) -> Result<Option<FrameId>> {
    let guard = slot().lock();
    let Some(ctx) = guard.as_ref() else {
        return Ok(None);
    };
    ctx.set_viewport(Vec2::new(pan_x, pan_y), zoom);
    let id = render_cached_scene_locked(ctx)?;
    drop(guard);
    Ok(id)
}

/// Repaint the cached scene with the renderer lock **already held**.
///
/// Shared by [`render_current`] and [`set_viewport_and_render`] so the
/// native/offscreen branch selection and the lock-order discipline live
/// in exactly one place and cannot drift between the two present entry
/// points. The caller already holds the `slot` guard and lends us `ctx`
/// borrowed from it; we acquire `native_slot` (native builds only) and
/// then `scene_slot`, preserving the canonical
/// `slot -> native_slot -> scene_slot` order. Both inner guards are
/// released — in reverse acquisition order — when this returns, before
/// the caller drops `slot`.
///
/// Returns `Ok(None)` when no scene has been published yet (nothing to
/// repaint); the renderer borrows the cached scene in place and never
/// clones it out.
fn render_cached_scene_locked(ctx: &RenderContext) -> Result<Option<FrameId>> {
    #[cfg(feature = "native_canvas")]
    {
        let native = native_slot().lock();
        if let Some(surface) = native.as_ref() {
            let scene_guard = scene_slot().lock();
            return match scene_guard.as_ref() {
                Some(scene) => Ok(Some(ctx.render_frame_native(scene, surface)?)),
                None => Ok(None),
            };
        }
    }
    let scene_guard = scene_slot().lock();
    match scene_guard.as_ref() {
        Some(scene) => {
            let id = ctx.render_frame(scene)?;
            // Publish into the shared-memory ring on the pan/zoom hot
            // path too (the full-frame-churn case the shared path
            // targets). `scene_slot` is held here, so this acquires the
            // strictly-inner `shared_present_slot` — canonical order
            // `slot -> scene_slot -> shared_present_slot`.
            #[cfg(feature = "native_canvas")]
            publish_shared_locked(ctx, id);
            Ok(Some(id))
        }
        None => Ok(None),
    }
}

/// Drop the cached scene without tearing down the renderer.
///
/// Called by [`crate::document::project_close`] so a closed project's
/// scene neither lingers in renderer memory nor can be repainted by a
/// later [`render_current`] (which would otherwise flash stale content
/// from the previous project before a new one is synced). The renderer
/// itself — and its last presented frame — stays intact so the host can
/// keep presenting until it unmounts the canvas.
///
/// Acquire-then-drop: holds only `scene_slot` for the duration of the
/// assignment, so it can never participate in a lock-order inversion.
pub fn clear_scene() {
    *scene_slot().lock() = None;
}

/// Snapshot of the latest published frame (RGBA8). Copies bytes out so
/// we don't hold the presenter's read lock across the N-API boundary.
pub fn get_frame_bytes() -> Result<Option<Vec<u8>>> {
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    let bytes = ctx.latest_frame().map(|lease| lease.pixels().to_vec());
    drop(guard);
    Ok(bytes)
}

pub fn get_frame_info() -> Result<Option<RendererFrameInfo>> {
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    let info = ctx.latest_frame().map(|lease| RendererFrameInfo {
        frame_id: lease.frame_id().0,
        width: lease.width(),
        height: lease.height(),
        byte_length: lease.pixels().len() as u32,
    });
    drop(guard);
    Ok(info)
}

/// Atomically snapshot the latest frame: bytes + metadata in a single
/// locked read.
///
/// This is the bridge's preferred frame-fetch path: it avoids the
/// `get_frame_info` + `get_frame_bytes` race window where the published
/// frame could roll over between the two calls (the host would see
/// `byte_length` from frame N but `bytes` from frame N+1, with
/// potentially different dimensions during a resize).
pub fn acquire_frame() -> Result<Option<AcquiredFrame>> {
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    let frame = ctx.latest_frame().map(|lease| AcquiredFrame {
        frame_id: lease.frame_id().0,
        width: lease.width(),
        height: lease.height(),
        bytes: lease.pixels().to_vec(),
    });
    drop(guard);
    Ok(frame)
}

/// Fraction of the framebuffer above which a dirty region is presented
/// as a full frame instead of a sub-rect. Past this point the bookkeeping
/// and a partial `putImageData` stop paying for themselves versus simply
/// shipping the whole frame, so we fall back to the full-frame copy.
const MAX_PARTIAL_PRESENT_FRACTION: f32 = 0.5;

/// Atomically snapshot the latest frame for presentation, returning only
/// the pixels that changed since the host last consumed a frame.
///
/// This is the dirty-rect present path that backs
/// `renderer_acquire_present`. On a typical edit the returned
/// [`AcquiredPresent`] carries just the changed sub-rect (`full == false`,
/// `bytes` sized `dirty_width * dirty_height * 4`); on the first frame,
/// after a resize, or when the change is large it carries the whole frame
/// (`full == true`). When nothing changed since the last consume it
/// returns a zero-sized dirty rect with empty `bytes` so the host can
/// cheaply skip the blit. Consuming also resets the accumulated dirty
/// region, so the host's present loop may safely coalesce frames.
pub fn acquire_present() -> Result<Option<AcquiredPresent>> {
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    let present = ctx
        .take_present(MAX_PARTIAL_PRESENT_FRACTION)
        .map(|snap| AcquiredPresent {
            frame_id: snap.frame_id.0,
            width: snap.width,
            height: snap.height,
            dirty_x: snap.dirty.x,
            dirty_y: snap.dirty.y,
            dirty_width: snap.dirty.width,
            dirty_height: snap.dirty.height,
            full: snap.full,
            bytes: snap.bytes,
        });
    drop(guard);
    Ok(present)
}

// -----------------------------------------------------------------------------
// Native canvas presentation path — Phase 1, Block A, Task 5.
//
// The bridge tracks an *optional* `NativeSurface`. When present, the
// next `render` call routes through `render_frame_native` and the
// pixels go straight to the swapchain — no CPU readback, no IPC
// `putImageData`. The default build does not compile the
// `native_canvas` feature, so the slot is permanently `None` and the
// only path is the offscreen → presenter → IPC chain.
// -----------------------------------------------------------------------------

#[cfg(feature = "native_canvas")]
fn native_slot() -> &'static Mutex<Option<kcreate_renderer::NativeSurface>> {
    static NATIVE: OnceLock<Mutex<Option<kcreate_renderer::NativeSurface>>> = OnceLock::new();
    NATIVE.get_or_init(|| Mutex::new(None))
}

/// Currently selected presentation mode. Used by both the renderer
/// path selection (`render_scene`) and the host UI's "Mode: Native /
/// Offscreen" badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationMode {
    Offscreen,
    Native,
}

impl PresentationMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offscreen => "offscreen",
            Self::Native => "native",
        }
    }
}

/// Probe the bridge's current presentation mode. Returns `Offscreen`
/// in default builds (no `native_canvas` feature) and in feature
/// builds when no native surface has been attached yet.
#[must_use]
pub fn presentation_mode() -> PresentationMode {
    #[cfg(feature = "native_canvas")]
    {
        if native_slot().lock().is_some() {
            return PresentationMode::Native;
        }
    }
    PresentationMode::Offscreen
}

/// Attach a native surface created from the raw handle bytes Electron
/// ferries via `BrowserWindow::getNativeWindowHandle()`. The renderer
/// must already be initialized via [`init`].
///
/// Returns the platform variant the bridge interpreted the bytes as
/// (`appkit` / `win32` / `x11` / `wayland`). Subsequent calls to
/// [`render`] route through the native path until
/// [`switch_offscreen`] is called.
#[cfg(feature = "native_canvas")]
pub fn switch_native(handle_bytes: &[u8], width: u32, height: u32) -> Result<String> {
    use crate::native_canvas;
    let handle = native_canvas::wrap_handle(handle_bytes)
        .map_err(|e| BridgeError::Renderer(kcreate_renderer::RendererError::Wgpu(e.to_string())))?;
    let platform = handle.platform();
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    let surface = ctx.create_native_surface(handle, width, height)?;
    drop(guard);
    *native_slot().lock() = Some(surface);
    Ok(platform.as_str().to_string())
}

/// Detach the native surface and revert to the offscreen path. No-op
/// if no surface is attached. The offscreen pipeline state is
/// preserved (the same `RenderContext` was driving both paths) so the
/// next `render` call resumes producing IPC frames immediately.
#[cfg(feature = "native_canvas")]
pub fn switch_offscreen() {
    *native_slot().lock() = None;
}

// -----------------------------------------------------------------------------
// Shared-memory frame handoff — push past the dirty-rect IPC ceiling.
//
// The offscreen path ships present bytes across the main → renderer IPC
// boundary every frame: a structured-clone copy whose cost scales with the
// changed payload. A single-edit present collapses to a few KiB via the
// dirty-rect path, but a full-frame change (pan / zoom / scroll) still moves
// the whole `width * height * 4` framebuffer — megabytes per frame at
// 1080p+ — and that copy is the remaining present ceiling.
//
// The shared-memory path removes that per-frame copy. The publisher (this
// process — Electron main) mmaps a small tmpfs-backed framebuffer *ring*
// and, after every offscreen render, writes the freshly published full
// frame into the next slot under a seqlock. The reader (the Electron
// renderer process, via a bridge `.node` it `process.dlopen`s locally)
// maps the same file and copies the latest frame straight into its
// `ImageData` backing buffer. Zero bytes cross IPC per frame; only a tiny
// `SharedPresentDescriptor` (path + geometry + slot count) is exchanged
// once, at handshake time.
//
// Everything here is behind `native_canvas` — the only module allowed the
// `unsafe` mmap (encapsulated in `native_canvas::{MappedRegion,
// MappedRegionRo}`; the ring logic in `shared_present` is `unsafe`-free).
// Default builds don't compile any of it, so the present path is exactly
// the Phase 0 offscreen → presenter → IPC chain and `CanvasHost` falls back
// to dirty-rect IPC with no behavior change.
//
// **Lock order.** `shared_present_slot` and `shared_reader_slot` are the
// strictly-innermost mutexes: `slot -> native_slot -> scene_slot ->
// shared_present_slot`. The publisher slot is only ever acquired last (the
// publish hook in `render_scene` / `render_cached_scene_locked`) or alone
// (enable / descriptor / disable / resize / shutdown). The reader slot is
// only ever acquired alone. So neither can participate in an inversion.
// -----------------------------------------------------------------------------

#[cfg(feature = "native_canvas")]
use crate::shared_present::{SharedFrameMeta, SharedFramePublisher, SharedFrameReader};

/// Process-wide shared-memory publisher (Electron **main** process). `Some`
/// once the host has handshaked via [`shared_present_enable`].
#[cfg(feature = "native_canvas")]
fn shared_present_slot() -> &'static Mutex<Option<SharedFramePublisher>> {
    static SHARED_PUBLISHER: OnceLock<Mutex<Option<SharedFramePublisher>>> = OnceLock::new();
    SHARED_PUBLISHER.get_or_init(|| Mutex::new(None))
}

/// Process-wide shared-memory reader (Electron **renderer** process). `Some`
/// once [`shared_reader_open`] has mapped a descriptor. Lives in a different
/// OS process from [`shared_present_slot`] in production; co-resident only in
/// single-process tests / benches, where they are distinct slots.
#[cfg(feature = "native_canvas")]
fn shared_reader_slot() -> &'static Mutex<Option<SharedFrameReader>> {
    static SHARED_READER: OnceLock<Mutex<Option<SharedFrameReader>>> = OnceLock::new();
    SHARED_READER.get_or_init(|| Mutex::new(None))
}

/// Copy the renderer's latest published full frame into the shared ring,
/// with the `slot` guard already held by the caller. No-op when no
/// publisher is attached, or when the published frame's geometry does not
/// match the ring (a resize is mid-flight and the host has not re-handshaked
/// yet) — the host keeps presenting via dirty-rect IPC until then.
///
/// Acquires only `shared_present_slot`, the innermost mutex, so it cannot
/// invert the canonical lock order regardless of which present entry point
/// called it.
#[cfg(feature = "native_canvas")]
fn publish_shared_locked(ctx: &RenderContext, frame_id: FrameId) {
    let mut guard = shared_present_slot().lock();
    let Some(publisher) = guard.as_mut() else {
        return;
    };
    let Some(lease) = ctx.latest_frame() else {
        return;
    };
    let (pw, ph) = publisher.dimensions();
    if lease.width() == pw && lease.height() == ph {
        publisher.publish_full(frame_id.0, lease.pixels());
    }
}

/// Enable (or re-handshake) the shared-memory present ring at the given
/// framebuffer geometry and return the descriptor the renderer process needs
/// to map it. Idempotent: if a publisher already exists at the same geometry
/// its existing descriptor is returned unchanged (so repeated handshakes
/// don't churn the tmpfs file); a geometry change drops the old ring and
/// creates a fresh one. The renderer must already be initialized.
///
/// Acquires only `shared_present_slot`.
#[cfg(feature = "native_canvas")]
pub fn shared_present_enable(width: u32, height: u32) -> Result<SharedPresentDescriptor> {
    let mut guard = shared_present_slot().lock();
    if let Some(publisher) = guard.as_ref() {
        let (pw, ph) = publisher.dimensions();
        if pw == width && ph == height {
            return Ok(publisher.descriptor().into());
        }
    }
    // Drop any stale-geometry publisher before creating the new one so the
    // old tmpfs file is unlinked promptly.
    *guard = None;
    let publisher = SharedFramePublisher::create(width, height, DEFAULT_SHARED_SLOT_COUNT)?;
    let descriptor = publisher.descriptor().into();
    *guard = Some(publisher);
    Ok(descriptor)
}

/// Current shared-present descriptor, or `None` when the ring is disabled.
#[cfg(feature = "native_canvas")]
#[must_use]
pub fn shared_present_descriptor() -> Option<SharedPresentDescriptor> {
    shared_present_slot()
        .lock()
        .as_ref()
        .map(|p| p.descriptor().into())
}

/// Whether a shared-memory publisher is currently attached.
#[cfg(feature = "native_canvas")]
#[must_use]
pub fn shared_present_enabled() -> bool {
    shared_present_slot().lock().is_some()
}

/// Disable the shared-memory present ring and unlink its backing file. No-op
/// when already disabled. The present path reverts to dirty-rect IPC.
#[cfg(feature = "native_canvas")]
pub fn shared_present_disable() {
    *shared_present_slot().lock() = None;
}

/// Open the shared-memory reader (Electron **renderer** process) against a
/// descriptor the main process produced via [`shared_present_enable`].
/// Replaces any previously opened reader (e.g. after a resize re-handshake).
///
/// Acquires only `shared_reader_slot`.
#[cfg(feature = "native_canvas")]
pub fn shared_reader_open(descriptor: &SharedPresentDescriptor) -> Result<()> {
    let reader = SharedFrameReader::open(&descriptor.clone().into())?;
    *shared_reader_slot().lock() = Some(reader);
    Ok(())
}

/// Pixel length (`width * height * 4`) of a frame in the currently open
/// reader, or `None` when no reader is open. Lets the host size its
/// `ImageData` backing buffer before the first read.
#[cfg(feature = "native_canvas")]
#[must_use]
pub fn shared_reader_frame_len() -> Option<usize> {
    shared_reader_slot()
        .lock()
        .as_ref()
        .map(SharedFrameReader::frame_len)
}

/// Whether a shared-memory reader is currently open.
#[cfg(feature = "native_canvas")]
#[must_use]
pub fn shared_reader_open_status() -> bool {
    shared_reader_slot().lock().is_some()
}

/// Read the newest shared-memory frame into `dest` (the caller's reusable
/// `ImageData` backing buffer), skipping the copy when the newest frame is
/// not newer than `since`. Returns the frame metadata on a fresh copy,
/// `Ok(None)` when nothing newer is available or no reader is open.
///
/// This is the single per-frame memcpy of the whole pipeline: shared ring →
/// `ImageData`, performed in the renderer process with zero IPC. Acquires
/// only `shared_reader_slot`.
#[cfg(feature = "native_canvas")]
pub fn shared_reader_read_into(
    since: Option<u64>,
    dest: &mut [u8],
) -> Result<Option<SharedFrameMeta>> {
    let mut guard = shared_reader_slot().lock();
    let Some(reader) = guard.as_mut() else {
        return Ok(None);
    };
    let meta = reader.read_new_into(since, dest)?;
    Ok(meta)
}

/// Close the shared-memory reader. No-op when none is open.
#[cfg(feature = "native_canvas")]
pub fn shared_reader_close() {
    *shared_reader_slot().lock() = None;
}

/// Number of slots in the shared-memory present ring. Three gives the reader
/// a stable slot to copy from while the publisher writes the next and a
/// third is in flight, so a reader never blocks the publisher.
#[cfg(feature = "native_canvas")]
const DEFAULT_SHARED_SLOT_COUNT: u32 = 3;

/// Descriptor the renderer process needs to map the shared-memory present
/// ring. Mirrors [`crate::shared_present::SharedPresentDescriptor`] in the
/// `state` DTO vocabulary (so `lib.rs` stays a thin marshalling layer); the
/// two convert via `From` in both directions.
#[cfg(feature = "native_canvas")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPresentDescriptor {
    pub path: String,
    pub len: u64,
    pub width: u32,
    pub height: u32,
    pub slot_count: u32,
}

#[cfg(feature = "native_canvas")]
impl From<crate::shared_present::SharedPresentDescriptor> for SharedPresentDescriptor {
    fn from(d: crate::shared_present::SharedPresentDescriptor) -> Self {
        Self {
            path: d.path,
            len: d.len,
            width: d.width,
            height: d.height,
            slot_count: d.slot_count,
        }
    }
}

#[cfg(feature = "native_canvas")]
impl From<SharedPresentDescriptor> for crate::shared_present::SharedPresentDescriptor {
    fn from(d: SharedPresentDescriptor) -> Self {
        Self {
            path: d.path,
            len: d.len,
            width: d.width,
            height: d.height,
            slot_count: d.slot_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererInfo {
    pub tier: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererFrameInfo {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub byte_length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquiredFrame {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// A frame snapshot for the dirty-rect present path.
///
/// `bytes` is either the whole frame (`full == true`,
/// `width * height * 4` bytes) or just the `dirty` sub-rect
/// (`full == false`, `dirty_width * dirty_height * 4` bytes, possibly
/// empty when nothing changed). The host patches its persistent
/// backbuffer with `bytes` and repaints with the matching
/// `putImageData` form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquiredPresent {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub dirty_x: u32,
    pub dirty_y: u32,
    pub dirty_width: u32,
    pub dirty_height: u32,
    pub full: bool,
    pub bytes: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn init_then_shutdown_round_trips() {
        reset_for_tests();
        let info = init(32, 16).expect("init");
        assert_eq!(info.width, 32);
        assert_eq!(info.height, 16);
        shutdown();
        // Re-init should succeed after shutdown.
        init(64, 64).expect("re-init");
        shutdown();
    }

    #[test]
    #[serial]
    fn re_init_same_size_returns_existing() {
        reset_for_tests();
        let a = init(8, 8).expect("init");
        let b = init(8, 8).expect("second init must succeed (idempotent)");
        assert_eq!(a, b);
        shutdown();
    }

    #[test]
    #[serial]
    fn re_init_different_size_resizes_in_place() {
        reset_for_tests();
        let a = init(8, 8).expect("init");
        assert_eq!((a.width, a.height), (8, 8));
        let b = init(16, 32).expect("second init must resize in place");
        assert_eq!((b.width, b.height), (16, 32));
        // Same renderer tier — GPU device was NOT torn down.
        assert_eq!(a.tier, b.tier);
        shutdown();
    }

    #[test]
    #[serial]
    fn operations_before_init_error() {
        reset_for_tests();
        let err = invalidate(None).expect_err("not initialized");
        assert!(matches!(err, BridgeError::NotInitialized));
        let err = resize(10, 10).expect_err("not initialized");
        assert!(matches!(err, BridgeError::NotInitialized));
        let err = set_viewport(0.0, 0.0, 1.0).expect_err("not initialized");
        assert!(matches!(err, BridgeError::NotInitialized));
    }

    #[test]
    #[serial]
    fn render_publishes_frame_and_info() {
        reset_for_tests();
        init(32, 16).expect("init");
        let scene_json = r#"{
            "clear_color": [0.0, 0.0, 0.0, 1.0],
            "objects": [{
                "id": 1, "z": 0, "translation": [0.0, 0.0],
                "style": { "fill": [1.0, 0.0, 0.0, 1.0], "stroke": null },
                "kind": { "type": "rect", "x": 4.0, "y": 4.0, "width": 8.0, "height": 8.0 }
            }]
        }"#;
        let id = render(scene_json).expect("render");
        assert!(id.0 > 0);

        let info = get_frame_info().expect("info").expect("some");
        assert_eq!(info.width, 32);
        assert_eq!(info.height, 16);
        assert_eq!(info.byte_length, 32 * 16 * 4);
        assert_eq!(info.frame_id, id.0);

        let bytes = get_frame_bytes().expect("bytes").expect("some");
        assert_eq!(bytes.len(), 32 * 16 * 4);

        shutdown();
    }

    #[test]
    #[serial]
    fn acquire_frame_returns_atomic_snapshot() {
        reset_for_tests();
        init(16, 8).expect("init");
        let scene_json = r#"{ "clear_color":[0,0,0,1], "objects": [] }"#;
        let id = render(scene_json).expect("render");
        let frame = acquire_frame().expect("acquire").expect("some");
        assert_eq!(frame.frame_id, id.0);
        assert_eq!(frame.width, 16);
        assert_eq!(frame.height, 8);
        assert_eq!(frame.bytes.len(), 16 * 8 * 4);
        shutdown();
    }

    #[test]
    #[serial]
    fn acquire_frame_before_first_render_is_none() {
        reset_for_tests();
        init(16, 8).expect("init");
        assert!(acquire_frame().expect("acquire").is_none());
        shutdown();
    }

    #[test]
    #[serial]
    fn acquire_present_before_first_render_is_none() {
        reset_for_tests();
        init(16, 8).expect("init");
        assert!(acquire_present().expect("acquire").is_none());
        shutdown();
    }

    #[test]
    #[serial]
    fn acquire_present_first_frame_is_full() {
        reset_for_tests();
        init(64, 64).expect("init");
        let scene_json = r#"{ "clear_color":[0,0,0,1], "objects": [] }"#;
        let id = render(scene_json).expect("render");
        let present = acquire_present().expect("acquire").expect("some");
        assert_eq!(present.frame_id, id.0);
        assert!(present.full, "first frame must be a full present");
        assert_eq!(present.dirty_width, 64);
        assert_eq!(present.dirty_height, 64);
        assert_eq!(present.bytes.len(), 64 * 64 * 4);
        shutdown();
    }

    #[test]
    #[serial]
    fn acquire_present_unchanged_redraw_is_empty() {
        reset_for_tests();
        init(64, 64).expect("init");
        let scene_json = r#"{ "clear_color":[0,0,0,1], "objects": [] }"#;
        render(scene_json).expect("render 1");
        // Consume the initial full frame.
        let _ = acquire_present().expect("acquire").expect("some");
        // Force a re-render of identical pixels.
        invalidate(None).expect("invalidate");
        render(scene_json).expect("render 2");
        let present = acquire_present().expect("acquire").expect("some");
        assert!(!present.full);
        assert_eq!(present.dirty_width, 0);
        assert_eq!(present.dirty_height, 0);
        assert!(present.bytes.is_empty());
        shutdown();
    }

    #[test]
    #[serial]
    fn acquire_present_small_edit_is_partial() {
        reset_for_tests();
        init(64, 64).expect("init");
        let empty = r#"{ "clear_color":[0,0,0,1], "objects": [] }"#;
        render(empty).expect("render 1");
        let _ = acquire_present().expect("acquire").expect("some");

        // Add a small opaque rect: a sub-region of the frame changes.
        let edited = r#"{
            "clear_color": [0.0, 0.0, 0.0, 1.0],
            "objects": [{
                "id": 1, "z": 0, "translation": [0.0, 0.0],
                "style": { "fill": [1.0, 0.0, 0.0, 1.0], "stroke": null },
                "kind": { "type": "rect", "x": 8.0, "y": 8.0, "width": 10.0, "height": 10.0 }
            }]
        }"#;
        invalidate(None).expect("invalidate");
        render(edited).expect("render 2");
        let present = acquire_present().expect("acquire").expect("some");
        assert!(
            !present.full,
            "a small edit must present a sub-rect, not the whole frame"
        );
        assert!(present.dirty_width > 0 && present.dirty_height > 0);
        // The changed region is the ~10x10 rect plus AA fringe — far
        // smaller than the 64x64 frame.
        assert!(
            present.dirty_width <= 16,
            "dirty width {} too large",
            present.dirty_width
        );
        assert!(
            present.dirty_height <= 16,
            "dirty height {} too large",
            present.dirty_height
        );
        assert_eq!(
            present.bytes.len(),
            present.dirty_width as usize * present.dirty_height as usize * 4
        );
        shutdown();
    }

    #[test]
    #[serial]
    fn invalidate_with_region_clamps_to_dirty_set() {
        reset_for_tests();
        init(64, 64).expect("init");
        invalidate(Some(Rect::new(10.0, 10.0, 5.0, 5.0))).expect("invalidate");
        // First render publishes; subsequent render with no further dirtying reuses the frame.
        let scene_json = r#"{ "clear_color":[0,0,0,1], "objects": [] }"#;
        let first = render(scene_json).expect("render 1");
        let second = render(scene_json).expect("render 2");
        assert_eq!(
            first, second,
            "no new dirty region should reuse the previous frame id"
        );
        shutdown();
    }

    /// Scene JSON used by the render-content guards: an 8×8 red rect on a
    /// black background, sized to land entirely inside a 32×16 target at
    /// the default (identity) viewport.
    const RED_RECT_ON_BLACK: &str = r#"{
        "clear_color": [0.0, 0.0, 0.0, 1.0],
        "objects": [{
            "id": 1, "z": 0, "translation": [0.0, 0.0],
            "style": { "fill": [1.0, 0.0, 0.0, 1.0], "stroke": null },
            "kind": { "type": "rect", "x": 4.0, "y": 4.0, "width": 8.0, "height": 8.0 }
        }]
    }"#;

    /// Count strongly-red pixels (object fill) in an RGBA8 buffer.
    fn count_red(bytes: &[u8]) -> usize {
        bytes
            .chunks_exact(4)
            .filter(|p| p[0] > 200 && p[1] < 50 && p[2] < 50)
            .count()
    }

    #[test]
    #[serial]
    fn render_current_before_any_render_is_none() {
        reset_for_tests();
        init(32, 16).expect("init");
        // Nothing has been published yet, so there is no scene to repaint.
        assert!(
            render_current().expect("render_current").is_none(),
            "render_current must be None before any scene is published"
        );
        shutdown();
    }

    #[test]
    #[serial]
    fn render_current_repaints_last_published_scene() {
        reset_for_tests();
        init(32, 16).expect("init");

        // Publish a scene, then prove it actually painted the object.
        let first = render(RED_RECT_ON_BLACK).expect("render");
        let before = acquire_frame().expect("acquire").expect("some");
        let red_before = count_red(&before.bytes);
        assert!(
            red_before > 0,
            "object render must produce non-background (red) pixels, got {red_before}"
        );

        // Without re-dirtying, the renderer reuses the cached frame, so
        // render_current returns the same id (no wasted GPU work).
        let cached = render_current()
            .expect("render_current")
            .expect("scene published");
        assert_eq!(
            cached, first,
            "render_current with no new dirty region reuses the cached frame id"
        );

        // After an invalidate, render_current must rebuild the SAME scene
        // (not an empty one): a new frame id, still carrying the object.
        invalidate(None).expect("invalidate");
        let repainted = render_current()
            .expect("render_current")
            .expect("scene still published");
        assert_ne!(
            repainted, first,
            "render_current after invalidate must publish a new frame"
        );
        let after = acquire_frame().expect("acquire").expect("some");
        assert_eq!(
            after.frame_id, repainted.0,
            "acquired frame id must match the repaint"
        );
        assert_eq!(
            count_red(&after.bytes),
            red_before,
            "repaint must reproduce the published scene, not a blank frame"
        );

        shutdown();
    }

    #[test]
    #[serial]
    fn set_viewport_and_render_repaints_on_viewport_change() {
        reset_for_tests();
        init(32, 16).expect("init");

        // Publish a scene so the combined call has something to repaint.
        let first = render(RED_RECT_ON_BLACK).expect("render");
        let before = acquire_frame().expect("acquire").expect("some");
        let red_before = count_red(&before.bytes);
        assert!(
            red_before > 0,
            "object render must produce non-background (red) pixels, got {red_before}"
        );

        // A viewport change marks the renderer dirty, so the combined
        // call publishes a NEW frame carrying the SAME scene (not a
        // blank one) — exactly what separate set_viewport + render_current
        // would have produced, but in one lock + one round-trip. We zoom
        // *out* (and keep pan at the origin) so the centred 8×8 rect
        // stays inside the 32×16 target and the repaint is still
        // observably non-blank.
        let base_zoom = viewport_zoom();
        let new_zoom = if (base_zoom - 0.75).abs() < 0.001 {
            0.5
        } else {
            0.75
        };
        let moved = set_viewport_and_render(0.0, 0.0, new_zoom)
            .expect("set_viewport_and_render")
            .expect("scene published");
        assert_ne!(
            moved, first,
            "a viewport change must publish a new frame id"
        );
        assert!(
            (viewport_zoom() - new_zoom).abs() < f32::EPSILON,
            "the viewport write must have been applied"
        );
        let after = acquire_frame().expect("acquire").expect("some");
        assert_eq!(
            after.frame_id, moved.0,
            "acquired frame id must match the combined repaint"
        );
        assert!(
            count_red(&after.bytes) > 0,
            "repaint must reproduce the published scene (red still \
             visible), not a blank frame"
        );

        // Re-issuing the SAME viewport leaves nothing dirty, so the
        // renderer reuses the cached frame id (no wasted GPU work).
        let cached = set_viewport_and_render(0.0, 0.0, new_zoom)
            .expect("set_viewport_and_render")
            .expect("scene still published");
        assert_eq!(
            cached, moved,
            "an unchanged viewport reuses the cached frame id"
        );

        shutdown();
    }

    #[test]
    #[serial]
    fn set_viewport_and_render_before_any_render_is_none() {
        reset_for_tests();
        init(32, 16).expect("init");
        // No scene published yet: the viewport write still takes effect,
        // but there is nothing to repaint, so the combined call returns
        // None (not an error) just like render_current.
        assert!(
            set_viewport_and_render(5.0, 7.0, 1.5)
                .expect("set_viewport_and_render")
                .is_none(),
            "set_viewport_and_render must be None before any scene is published"
        );
        assert!(
            (viewport_zoom() - 1.5).abs() < f32::EPSILON,
            "the viewport write must apply even with no scene to repaint"
        );
        shutdown();
    }

    #[test]
    #[serial]
    fn set_viewport_and_render_without_renderer_is_none() {
        reset_for_tests();
        // No init: present-path calls degrade to Ok(None) rather than
        // erroring, mirroring render_current's headless contract.
        assert!(
            set_viewport_and_render(0.0, 0.0, 1.0)
                .expect("set_viewport_and_render")
                .is_none(),
            "set_viewport_and_render must be None when no renderer is attached"
        );
    }

    #[test]
    #[serial]
    fn clear_scene_drops_cached_scene_so_render_current_is_none() {
        // `project_close` calls `clear_scene` so a closed project's scene
        // can't be repainted by a later `render_current`. The renderer
        // stays attached; only the cached scene is dropped.
        reset_for_tests();
        init(32, 16).expect("init");
        render(RED_RECT_ON_BLACK).expect("render");
        // Sanity: the scene is cached and repaintable.
        assert!(
            render_current().expect("render_current").is_some(),
            "a published scene must be repaintable before clear_scene"
        );

        clear_scene();

        // After clearing, there is nothing to repaint even though the
        // renderer is still initialized (so this is `Ok(None)`, not an
        // error).
        assert!(
            render_current().expect("render_current").is_none(),
            "render_current must be None after clear_scene drops the cached scene"
        );
        shutdown();
    }

    #[test]
    #[serial]
    fn render_paints_non_background_pixels() {
        // Golden guard against the blank-canvas regression: rendering a
        // scene with an object must yield pixels that differ from the
        // clear color. A frame that is entirely the background color
        // (the symptom of the WS-2 bug) fails here.
        reset_for_tests();
        init(32, 16).expect("init");
        render(RED_RECT_ON_BLACK).expect("render");
        let frame = acquire_frame().expect("acquire").expect("some");

        let total = (frame.width * frame.height) as usize;
        let red = count_red(&frame.bytes);
        assert!(
            red > 0,
            "expected the red rect to paint at least one pixel, got 0 of {total}"
        );
        assert!(
            red < total,
            "expected background pixels to remain, but all {total} pixels were the object color"
        );

        shutdown();
    }

    // ---- shared-memory present ring (native_canvas feature) -----------
    //
    // These exercise the state-machine behavior added for the shared
    // present handoff. The publisher and reader live in the same process
    // here (co-resident); in production they are in the Electron main and
    // renderer processes respectively, mapping the same backing file.

    #[cfg(feature = "native_canvas")]
    #[test]
    #[serial]
    fn shared_present_round_trips_full_frame() {
        reset_for_tests();
        init(32, 16).expect("init");
        let descriptor = shared_present_enable(32, 16).expect("enable shared present");
        assert_eq!((descriptor.width, descriptor.height), (32, 16));
        shared_reader_open(&descriptor).expect("open reader");

        // Render publishes the full frame into the ring after rasterising.
        let id = render(RED_RECT_ON_BLACK).expect("render");

        let frame_len = shared_reader_frame_len().expect("reader open");
        assert_eq!(frame_len, 32 * 16 * 4);
        let mut dest = vec![0u8; frame_len];
        let meta = shared_reader_read_into(None, &mut dest)
            .expect("read")
            .expect("a fresh frame is available");
        assert_eq!(meta.frame_id, id.0);
        assert_eq!((meta.width, meta.height), (32, 16));
        assert!(meta.full, "publisher always ships full frames");

        // The shared read must be byte-identical to the IPC frame bytes —
        // the two paths present the very same framebuffer.
        let ipc_bytes = get_frame_bytes().expect("bytes").expect("some");
        assert_eq!(dest, ipc_bytes, "shared frame must match the IPC frame");

        // The red rect actually painted (not a blank frame).
        assert!(
            count_red(&dest) > 0,
            "shared frame carries the rendered rect"
        );

        shutdown();
    }

    #[cfg(feature = "native_canvas")]
    #[test]
    #[serial]
    fn shared_present_enable_is_idempotent_and_resizes() {
        reset_for_tests();
        init(32, 16).expect("init");
        let a = shared_present_enable(32, 16).expect("enable");
        let b = shared_present_enable(32, 16).expect("re-enable same geometry");
        // Same geometry reuses the existing ring (same backing file).
        assert_eq!(a.path, b.path);
        assert_eq!((a.width, a.height), (b.width, b.height));

        // A geometry change drops the old ring and creates a fresh one.
        let c = shared_present_enable(64, 48).expect("enable new geometry");
        assert_ne!(c.path, a.path, "geometry change must rotate the ring file");
        assert_eq!((c.width, c.height), (64, 48));
        let current = shared_present_descriptor().expect("descriptor present");
        assert_eq!((current.width, current.height), (64, 48));

        shutdown();
    }

    #[cfg(feature = "native_canvas")]
    #[test]
    #[serial]
    fn shared_present_disable_reverts_to_ipc() {
        reset_for_tests();
        init(16, 16).expect("init");
        assert!(!shared_present_enabled());
        shared_present_enable(16, 16).expect("enable");
        assert!(shared_present_enabled());

        shared_present_disable();
        assert!(!shared_present_enabled());
        assert!(shared_present_descriptor().is_none());

        // Rendering still works with the ring disabled (dirty-rect IPC
        // fallback path) — no regression.
        let id = render(RED_RECT_ON_BLACK).expect("render after disable");
        assert!(id.0 > 0);

        shutdown();
    }

    #[cfg(feature = "native_canvas")]
    #[test]
    #[serial]
    fn shared_reader_skips_when_not_newer() {
        reset_for_tests();
        init(16, 16).expect("init");
        let descriptor = shared_present_enable(16, 16).expect("enable");
        shared_reader_open(&descriptor).expect("open reader");
        let id = render(RED_RECT_ON_BLACK).expect("render");

        let mut dest = vec![0u8; shared_reader_frame_len().expect("open")];
        let first = shared_reader_read_into(None, &mut dest)
            .expect("read")
            .expect("frame available");
        assert_eq!(first.frame_id, id.0);

        // Asking for frames strictly newer than the one we just read yields
        // nothing until the publisher advances — no redundant copy.
        let again = shared_reader_read_into(Some(first.frame_id), &mut dest).expect("read");
        assert!(again.is_none(), "no copy when nothing newer is published");

        shutdown();
    }

    #[cfg(feature = "native_canvas")]
    #[test]
    #[serial]
    fn resize_drops_shared_publisher_for_rehandshake() {
        reset_for_tests();
        init(32, 16).expect("init");
        let old = shared_present_enable(32, 16).expect("enable");
        assert!(shared_present_enabled());

        // A resize invalidates the old framebuffer geometry, so the main
        // process drops the publisher (unlinking its file). The present
        // path falls back to dirty-rect IPC until the host re-handshakes.
        // (The renderer-process reader is re-opened by `CanvasHost`, not by
        // this main-process call, so it is intentionally left untouched.)
        resize(64, 48).expect("resize");
        assert!(!shared_present_enabled(), "publisher dropped on resize");
        assert!(shared_present_descriptor().is_none());

        // Re-handshaking at the new geometry yields a fresh ring the
        // renderer can re-open.
        let new = shared_present_enable(64, 48).expect("re-enable at new size");
        assert_ne!(new.path, old.path, "resize rotates the ring file");
        assert_eq!((new.width, new.height), (64, 48));
        shared_reader_open(&new).expect("reader re-opens at new geometry");
        assert_eq!(shared_reader_frame_len(), Some(64 * 48 * 4));

        shutdown();
    }

    #[test]
    #[serial]
    fn scene_object_count_tracks_rendered_scene() {
        reset_for_tests();
        // No scene published yet → zero, never panics.
        assert_eq!(scene_object_count(), 0);

        init(32, 16).expect("init");
        assert_eq!(scene_object_count(), 0);

        let scene_json = r#"{
            "clear_color": [0.0, 0.0, 0.0, 1.0],
            "objects": [
                { "id": 1, "z": 0, "translation": [0.0, 0.0],
                  "style": { "fill": [1.0, 0.0, 0.0, 1.0], "stroke": null },
                  "kind": { "type": "rect", "x": 0.0, "y": 0.0, "width": 4.0, "height": 4.0 } },
                { "id": 2, "z": 1, "translation": [0.0, 0.0],
                  "style": { "fill": [0.0, 1.0, 0.0, 1.0], "stroke": null },
                  "kind": { "type": "rect", "x": 4.0, "y": 4.0, "width": 4.0, "height": 4.0 } }
            ]
        }"#;
        render(scene_json).expect("render");
        assert_eq!(scene_object_count(), 2);
        shutdown();
    }

    #[cfg(feature = "dev_seed")]
    #[test]
    #[serial]
    fn seed_dense_scene_renders_and_counts() {
        reset_for_tests();
        init(256, 256).expect("init");

        let (frame_id, object_count) = seed_dense_scene(5000, 1920.0, 1080.0).expect("seed");
        assert!(frame_id.0 > 0, "seeding publishes a real frame");
        // The builder lands near the target; assert it is genuinely dense
        // and that the count matches what the renderer now holds.
        assert!(
            object_count >= 4000,
            "seeded scene is dense: {object_count}"
        );
        assert_eq!(scene_object_count(), object_count);

        // The seeded scene is cached, so a viewport change re-renders it
        // (the pan/zoom path the HUD capture exercises) and advances the id.
        let panned = set_viewport_and_render(120.0, 80.0, 1.5)
            .expect("re-render at new viewport")
            .expect("a scene is cached");
        assert!(panned.0 > frame_id.0, "pan/zoom produces a fresh frame");
        assert_eq!(
            scene_object_count(),
            object_count,
            "object count is stable across pan"
        );

        shutdown();
    }
}
