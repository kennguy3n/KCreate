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
pub fn shutdown() {
    #[cfg(feature = "native_canvas")]
    {
        *native_slot().lock() = None;
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
    }
    *slot().lock() = None;
    *scene_slot().lock() = None;
}

pub fn resize(width: u32, height: u32) -> Result<()> {
    let mut guard = slot().lock();
    let ctx = guard.as_mut().ok_or(BridgeError::NotInitialized)?;
    ctx.resize(width, height)?;
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
}
