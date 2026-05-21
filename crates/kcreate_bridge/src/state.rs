//! Renderer state machine, decoupled from N-API.
//!
//! Everything here is plain Rust — testable with `cargo test` and reusable
//! by non-N-API consumers (e.g. headless tooling). The N-API wrappers in
//! `lib.rs` are thin: argument marshalling, status conversion, that's it.

use std::sync::OnceLock;

use kcreate_renderer::{initialize as renderer_initialize, FrameId, Rect, RenderContext, Vec2};
use parking_lot::Mutex;
use thiserror::Error;

use crate::wire::{parse_scene, WireError};

/// Errors from the bridge layer.
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("renderer not initialized — call renderer_init first")]
    NotInitialized,
    #[error("renderer already initialized — call renderer_shutdown first")]
    AlreadyInitialized,
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Renderer(#[from] kcreate_renderer::RendererError),
}

pub type Result<T> = std::result::Result<T, BridgeError>;

/// Static singleton state. One renderer per process.
fn slot() -> &'static Mutex<Option<RenderContext>> {
    static RENDERER: OnceLock<Mutex<Option<RenderContext>>> = OnceLock::new();
    RENDERER.get_or_init(|| Mutex::new(None))
}

/// Initialize the renderer at the given size. Errors if already initialized.
pub fn init(width: u32, height: u32) -> Result<RendererInfo> {
    let mut guard = slot().lock();
    if guard.is_some() {
        return Err(BridgeError::AlreadyInitialized);
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
    *slot().lock() = None;
}

/// Test-only helper: reset state so each test starts clean. Not exposed
/// via N-API.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *slot().lock() = None;
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
    let guard = slot().lock();
    let ctx = guard.as_ref().ok_or(BridgeError::NotInitialized)?;
    let id = ctx.render_frame(&scene)?;
    drop(guard);
    Ok(id)
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
    fn double_init_errors() {
        reset_for_tests();
        init(8, 8).expect("init");
        let err = init(8, 8).expect_err("should reject");
        assert!(
            matches!(err, BridgeError::AlreadyInitialized),
            "got {err:?}"
        );
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
