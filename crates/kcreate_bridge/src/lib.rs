//! N-API bridge: exposes the renderer to the Electron main/renderer
//! processes.
//!
//! The N-API surface in this file is intentionally thin — every function
//! is a one-shot wrapper around [`crate::state`], which holds the actual
//! renderer state machine. This separation lets us:
//!   1. Unit-test the full bridge behavior with plain `cargo test` (no
//!      Node.js process required), because `state` has no `napi`
//!      dependencies.
//!   2. Reuse the same logic from headless tooling and benchmarks.
//!
//! Frames are returned as Node.js `Buffer` values containing a freshly
//! copied snapshot of the latest published frame so JS code can hand
//! them to `ctx.putImageData()` / `createImageBitmap()` without holding
//! the presenter's read lock across the boundary.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod state;
pub mod wire;

use kcreate_renderer::Rect;
use napi::bindgen_prelude::{Buffer, Error as NapiError, Result as NapiResult, Status};
use napi_derive::napi;

use crate::state::{
    BridgeError, RendererFrameInfo as CoreFrameInfo, RendererInfo as CoreRendererInfo,
};

// Taken by value so it can be passed directly to `Result::map_err`.
#[allow(clippy::needless_pass_by_value)]
fn map_err(e: BridgeError) -> NapiError {
    let status = match e {
        BridgeError::NotInitialized | BridgeError::AlreadyInitialized => Status::InvalidArg,
        _ => Status::GenericFailure,
    };
    NapiError::new(status, format!("kcreate_bridge: {e}"))
}

/// Renderer info returned from [`renderer_init`].
#[napi(object)]
#[derive(Debug, Clone)]
pub struct RendererInfo {
    pub tier: String,
    pub width: u32,
    pub height: u32,
}

impl From<CoreRendererInfo> for RendererInfo {
    fn from(c: CoreRendererInfo) -> Self {
        Self {
            tier: c.tier,
            width: c.width,
            height: c.height,
        }
    }
}

/// Initialize the offscreen renderer.
#[napi]
pub fn renderer_init(width: u32, height: u32) -> NapiResult<RendererInfo> {
    state::init(width, height).map(Into::into).map_err(map_err)
}

/// Tear down the renderer. Idempotent.
#[napi]
pub fn renderer_shutdown() {
    state::shutdown();
}

/// Resize the offscreen target.
#[napi]
pub fn renderer_resize(width: u32, height: u32) -> NapiResult<()> {
    state::resize(width, height).map_err(map_err)
}

/// Update viewport pan + zoom.
#[napi]
pub fn renderer_set_viewport(pan_x: f64, pan_y: f64, zoom: f64) -> NapiResult<()> {
    state::set_viewport(pan_x as f32, pan_y as f32, zoom as f32).map_err(map_err)
}

/// Mark a region (or the entire canvas, if no rect is supplied) as dirty.
#[napi]
pub fn renderer_invalidate(
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
) -> NapiResult<()> {
    let region = match (x, y, width, height) {
        (Some(x), Some(y), Some(w), Some(h)) => {
            Some(Rect::new(x as f32, y as f32, w as f32, h as f32))
        }
        _ => None,
    };
    state::invalidate(region).map_err(map_err)
}

/// Render a JS-supplied scene description.
///
/// The scene is encoded as JSON because:
///   1. The Electron renderer process produces scenes in TS/JS, so JSON
///      is the lowest-friction wire format.
///   2. A stable JSON schema keeps the bridge surface narrow and
///      versionable.
///   3. We can switch the encoding to `MessagePack` or a binary `FlatBuffer`
///      later without changing the renderer.
///
/// Returns the published `frameId`.
//
// Takes an owned `String` because that's the type `#[napi]` accepts for
// JS string arguments; we immediately borrow it when handing off to the
// JSON parser.
#[allow(clippy::needless_pass_by_value)]
#[napi]
pub fn renderer_render(scene_json: String) -> NapiResult<u32> {
    let id = state::render(&scene_json).map_err(map_err)?;
    Ok(id.0 as u32)
}

/// Returns the latest published frame as an RGBA8 `Buffer`, or `null`
/// if no frame has been rendered yet.
#[napi]
pub fn renderer_get_frame() -> NapiResult<Option<Buffer>> {
    let bytes = state::get_frame_bytes().map_err(map_err)?;
    Ok(bytes.map(Buffer::from))
}

/// Metadata about the latest published frame.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct FrameInfo {
    pub frame_id: u32,
    pub width: u32,
    pub height: u32,
    pub byte_length: u32,
}

impl From<CoreFrameInfo> for FrameInfo {
    fn from(c: CoreFrameInfo) -> Self {
        Self {
            frame_id: c.frame_id as u32,
            width: c.width,
            height: c.height,
            byte_length: c.byte_length,
        }
    }
}

#[napi]
pub fn renderer_frame_info() -> NapiResult<Option<FrameInfo>> {
    state::get_frame_info()
        .map(|opt| opt.map(Into::into))
        .map_err(map_err)
}
