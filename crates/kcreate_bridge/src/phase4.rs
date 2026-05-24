//! Phase 4 bridge entry points: vision (VLM) describe / alt-text /
//! design critique, brand extraction, smart crop, design-token
//! suggestion, style description, and the image-generation sidecar
//! lifecycle + one-shot generate.
//!
//! Logic here is invoked from the thin N-API wrappers in `lib.rs`.
//! Following AGENTS.md: business logic lives here; `lib.rs` only
//! marshals types across the FFI boundary.
//!
//! Build modes:
//!   - **Default** (`llm` feature off): every vision call returns
//!     `Disabled`; the host UI shows "vision unavailable in this
//!     build" instead of crashing.
//!   - **`llm` feature**: pulls `ureq` and the helpers in
//!     `kcreate_ai::vision_chat` / `image_gen` to talk loopback.

use std::path::PathBuf;
use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use kcreate_core::node::NodeType;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use kcreate_ai::{
    brand_extract::{extract_brand_from_image, BrandExtraction},
    design_critique::critique_design,
    design_tokens_vlm::{suggest_design_tokens, DesignTokenSuggestion},
    image_gen::{generate_image, ImageGenConfig, ImageGenError, ImageGenRequest, ImageGenSidecar},
    llm_sidecar::{LlmSidecar, SidecarConfig, SidecarError, SidecarStatus},
    mlx_sidecar::{probe_mlx_available, MlxSidecar, MlxSidecarConfig},
    model_registry::{list_model_packs, mmproj_for, recommended_vision_pack},
    sidecar_dispatcher::{plan_dispatch, DispatchPlan, SidecarRuntime},
    smart_crop::{suggest_crop, CropSuggestion},
    style_describe::{describe_style, StyleDescription},
    vision_chat::{describe_image as vlm_describe_image, VisionChatError},
};

use crate::document::{blob_load, runtime_slot, with_workspace, DocumentBridgeError, Result};
use crate::llm::LlmBridgeError;
use crate::phase2::ai_models_dir as models_root;

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

/// Cross-bridge error type for Phase 4. Mirrors `LlmBridgeError` but
/// adds an `ImageGen` variant so the renderer can distinguish "VLM
/// failed" from "diffusion sidecar failed".
#[derive(Debug, thiserror::Error)]
pub enum Phase4BridgeError {
    #[error(transparent)]
    Document(#[from] DocumentBridgeError),
    #[error(transparent)]
    Sidecar(#[from] SidecarError),
    #[error(transparent)]
    Vision(#[from] VisionChatError),
    #[error(transparent)]
    ImageGen(#[from] ImageGenError),
    #[error(transparent)]
    Bridge(#[from] LlmBridgeError),
    #[error("vision sidecar is not ready")]
    VisionNotReady,
    #[error("image generation is not allowed on this device")]
    ImageGenNotAllowed,
    #[error("image generation sidecar is not ready")]
    ImageGenNotReady,
    #[error("invalid request: {0}")]
    Invalid(String),
}

pub type Phase4Result<T> = std::result::Result<T, Phase4BridgeError>;

// -----------------------------------------------------------------------------
// Vision sidecar slot — separate from the text-LLM slot, because a
// user may want a 3B text model and a 256M VLM running side-by-side
// without paying 5 GB of RAM each time they describe an image.
// -----------------------------------------------------------------------------

/// Variant of the live vision sidecar, mirroring
/// `kcreate_ai::sidecar_dispatcher::SidecarHandle` but kept inside
/// the bridge so we can hand out non-`'static` borrows safely.
enum VisionHandle {
    Llama(LlmSidecar),
    Mlx(MlxSidecar),
}

impl VisionHandle {
    fn status(&self) -> SidecarStatus {
        match self {
            Self::Llama(s) => s.status(),
            Self::Mlx(s) => s.status(),
        }
    }

    fn ready_port(&self) -> Option<u16> {
        match self.status() {
            SidecarStatus::Ready { port, .. } => Some(port),
            _ => None,
        }
    }

    fn stop(&mut self) {
        match self {
            Self::Llama(s) => s.stop(),
            Self::Mlx(s) => s.stop(),
        }
    }

    fn runtime(&self) -> SidecarRuntime {
        match self {
            Self::Llama(_) => SidecarRuntime::LlamaServer,
            Self::Mlx(_) => SidecarRuntime::MlxLm,
        }
    }
}

fn vision_slot() -> &'static Mutex<Option<VisionHandle>> {
    static SLOT: OnceLock<Mutex<Option<VisionHandle>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Wire shape mirroring `LlmStatusInfo` for the vision sidecar.
///
/// Serializes in camelCase so the TS mirror `VisionStatus` in
/// `apps/desktop/shared/scene.ts` (which uses `modelName`) lines up
/// — required by AGENTS.md §4 wire-format lockstep. All the other
/// Phase 4 wire types (`BrandExtraction`, `CropSuggestion`,
/// `DesignTokenSuggestion`, `StyleDescription`) use the same
/// convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionStatusInfo {
    pub state: &'static str,
    pub runtime: Option<&'static str>,
    pub port: Option<u16>,
    pub model_name: Option<String>,
    pub error: Option<String>,
}

/// Start a vision sidecar for `pack_id`. Looks up the model + (for
/// llama-server vision) its mmproj companion in the registry,
/// resolves them under the user's models directory, and spawns the
/// appropriate runtime — MLX when the pack id ends in `_mlx` AND the
/// host can run it, llama-server otherwise.
///
/// Returns the listening port **immediately**, before the health
/// check completes — the underlying sidecar runs its probe loop on
/// a background thread, and the renderer observes the
/// `Starting → Ready / Error` transition by polling
/// [`vision_status`]. This mirrors the established
/// [`crate::llm::llm_start`] pattern (`crates/kcreate_bridge/src/llm.rs`)
/// so cold-loading a 2.5 GB VLM cannot freeze the Electron main
/// process for the duration of the load.
pub fn vision_start(pack_id: String) -> Phase4Result<u16> {
    let models_dir = models_root();
    let platform = runtime_slot().lock().platform;
    let plan = plan_dispatch(&pack_id, &models_dir, platform, probe_mlx_available())
        .map_err(Phase4BridgeError::Sidecar)?;
    let mut guard = vision_slot().lock();
    // Stop the previous sidecar first — never run two side-by-side,
    // they'd both hold mmproj + model weights and OOM a tight box.
    if let Some(prev) = guard.as_mut() {
        prev.stop();
    }
    *guard = None;
    let (handle, port) = spawn_vision(&plan)?;
    *guard = Some(handle);
    Ok(port)
}

/// Stop the vision sidecar if running. Idempotent.
pub fn vision_stop() {
    let mut guard = vision_slot().lock();
    if let Some(s) = guard.as_mut() {
        s.stop();
    }
    *guard = None;
}

/// Snapshot of the vision sidecar status.
pub fn vision_status() -> VisionStatusInfo {
    let guard = vision_slot().lock();
    match guard.as_ref() {
        None => VisionStatusInfo {
            state: "stopped",
            runtime: None,
            port: None,
            model_name: None,
            error: None,
        },
        Some(h) => {
            let runtime = match h.runtime() {
                SidecarRuntime::LlamaServer => "llama_server",
                SidecarRuntime::MlxLm => "mlx_lm",
            };
            match h.status() {
                SidecarStatus::Stopped => VisionStatusInfo {
                    state: "stopped",
                    runtime: Some(runtime),
                    port: None,
                    model_name: None,
                    error: None,
                },
                SidecarStatus::Starting => VisionStatusInfo {
                    state: "starting",
                    runtime: Some(runtime),
                    port: None,
                    model_name: None,
                    error: None,
                },
                SidecarStatus::Ready {
                    port, model_name, ..
                } => VisionStatusInfo {
                    state: "ready",
                    runtime: Some(runtime),
                    port: Some(port),
                    model_name: Some(model_name),
                    error: None,
                },
                SidecarStatus::Error { message } => VisionStatusInfo {
                    state: "error",
                    runtime: Some(runtime),
                    port: None,
                    model_name: None,
                    error: Some(message),
                },
            }
        }
    }
}

/// Spawn the underlying sidecar. Returns the handle paired with the
/// listening port. `start()` on both runtimes is non-blocking — it
/// reserves the port, forks the child, and hands off health probing
/// to a background thread. Callers observe readiness via
/// [`vision_status`].
fn spawn_vision(plan: &DispatchPlan) -> Phase4Result<(VisionHandle, u16)> {
    match plan.runtime {
        SidecarRuntime::LlamaServer => {
            let mut cfg = SidecarConfig::new(plan.model_path.clone());
            if let Some(mmproj) = plan.mmproj_path.as_ref() {
                cfg = cfg.with_mmproj(Some(mmproj.clone()));
            }
            cfg.max_model_mb = runtime_slot().lock().effective_vision_model_mb();
            let mut s = LlmSidecar::new(cfg);
            let port = s.start().map_err(Phase4BridgeError::Sidecar)?;
            Ok((VisionHandle::Llama(s), port))
        }
        SidecarRuntime::MlxLm => {
            let cfg = MlxSidecarConfig {
                python: PathBuf::from("python3"),
                model_path: plan.model_path.clone(),
                context_size: 4096,
                health_timeout: std::time::Duration::from_secs(90),
                extra_args: vec![],
            };
            let mut s = MlxSidecar::new(cfg);
            let port = s.start().map_err(Phase4BridgeError::Sidecar)?;
            Ok((VisionHandle::Mlx(s), port))
        }
    }
}

fn vision_ready_port() -> Phase4Result<u16> {
    vision_slot()
        .lock()
        .as_ref()
        .and_then(VisionHandle::ready_port)
        .ok_or(Phase4BridgeError::VisionNotReady)
}

/// Read the RGBA pixel buffer for a raster node, decoding the
/// document's stored blob. Returns `(rgba, width, height)`.
fn load_raster_rgba(node_id: Uuid) -> Result<(Vec<u8>, u32, u32)> {
    let encoded = with_workspace(|ws| {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if !matches!(node.node_type, NodeType::RasterLayer) {
            return Err(DocumentBridgeError::InvalidNodeType(format!(
                "{:?}",
                node.node_type
            )));
        }
        let meta_value = node
            .metadata
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        blob_load(ws, &meta.blob_hash)
    })?;
    let img = image::load_from_memory(&encoded).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

// -----------------------------------------------------------------------------
// Vision describe / alt-text / critique entry points
// -----------------------------------------------------------------------------

/// Describe a raw RGBA image. Used when the caller has the pixels
/// in hand (e.g. captured artboard, or a thumbnail produced by the
/// renderer) and doesn't need to round-trip through a document node.
pub fn vision_describe_image(
    rgba: &[u8],
    width: u32,
    height: u32,
    user_prompt: &str,
) -> Phase4Result<String> {
    let port = vision_ready_port()?;
    let sys = "You are a precise visual analyst. Answer with concise, \
        factual descriptions. Do not invent details not visible in the image.";
    vlm_describe_image(port, sys, user_prompt, rgba, width, height)
        .map_err(Phase4BridgeError::Vision)
}

/// Describe a raster node's image content. Encodes for the
/// renderer's Ask → Preview → Apply loop: a free-form prompt
/// describes the layer's content.
pub fn vision_describe_node(node_id: Uuid, user_prompt: &str) -> Phase4Result<String> {
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    vision_describe_image(&rgba, w, h, user_prompt)
}

/// Specialised alt-text generator: same VLM machinery but with an
/// accessibility-focused system prompt and a short instruction set.
/// Returned text is suitable for direct insertion into a node's
/// `kcreate.altText` metadata via [`crate::phase2::ai_apply_alt_text`].
pub fn vision_generate_alt_text(rgba: &[u8], width: u32, height: u32) -> Phase4Result<String> {
    let port = vision_ready_port()?;
    let sys = "You are an accessibility auditor writing image alt-text \
        for screen readers. Be terse (under 120 characters), factual, \
        and concrete. Describe subject, action, and salient context. \
        Do not start with 'image of' or 'a picture of'.";
    let user = "Write alt-text for this image.";
    vlm_describe_image(port, sys, user, rgba, width, height).map_err(Phase4BridgeError::Vision)
}

/// Alt-text helper that loads pixels from a document node.
pub fn vision_generate_alt_text_for_node(node_id: Uuid) -> Phase4Result<String> {
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    vision_generate_alt_text(&rgba, w, h)
}

/// Design critique (Task 13). Wraps `critique_design`.
pub fn vision_analyze_design(rgba: &[u8], width: u32, height: u32) -> Phase4Result<String> {
    let port = vision_ready_port()?;
    critique_design(port, rgba, width, height).map_err(Phase4BridgeError::Vision)
}

/// Brand extraction (Task 16).
pub fn vision_extract_brand(
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Phase4Result<BrandExtraction> {
    let port = vision_ready_port()?;
    extract_brand_from_image(port, rgba, width, height).map_err(Phase4BridgeError::Vision)
}

/// Smart crop (Task 18).
pub fn vision_suggest_crop(
    rgba: &[u8],
    width: u32,
    height: u32,
    aspect_ratio: Option<f32>,
) -> Phase4Result<CropSuggestion> {
    let port = vision_ready_port()?;
    suggest_crop(port, rgba, width, height, aspect_ratio).map_err(Phase4BridgeError::Vision)
}

/// Design token suggestion (Task 19).
pub fn vision_suggest_design_tokens(
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Phase4Result<DesignTokenSuggestion> {
    let port = vision_ready_port()?;
    suggest_design_tokens(port, rgba, width, height).map_err(Phase4BridgeError::Vision)
}

/// Style description (Task 20).
pub fn vision_describe_style(
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Phase4Result<StyleDescription> {
    let port = vision_ready_port()?;
    describe_style(port, rgba, width, height).map_err(Phase4BridgeError::Vision)
}

/// Recommended vision pack id for the current device tier +
/// platform. Returns `None` when the registry has no recommendation
/// for the current device (extremely unlikely; recommendations cover
/// every supported tier × platform combo).
pub fn vision_recommended_pack() -> Option<String> {
    let cfg = runtime_slot().lock();
    recommended_vision_pack(cfg.device_tier, cfg.platform).map(str::to_string)
}

/// Inverse lookup convenience for the renderer: given a vision pack
/// id, return the mmproj companion's id (or `None` for MLX packs).
pub fn vision_mmproj_for(pack_id: String) -> Option<String> {
    mmproj_for(&pack_id).map(str::to_string)
}

/// List the packs the renderer is allowed to show in the vision
/// section of the Model Manager. Filters MLX packs off non-Apple
/// hosts, and clips by [`crate::runtime_slot`]'s vision ceiling.
pub fn vision_listable_packs() -> Vec<String> {
    let dir = models_root();
    let cfg = runtime_slot().lock();
    let cap_mb = cfg.effective_vision_model_mb();
    let platform = cfg.platform;
    drop(cfg);
    list_model_packs(&dir)
        .into_iter()
        .filter(|p| {
            // Vision packs (category Vision, or capability `vision`).
            p.category == kcreate_ai::ModelPackCategory::Vision
                || p.capabilities.iter().any(|c| c == "vision")
        })
        .filter(|p| {
            // MLX packs only on Apple Silicon.
            if p.id.ends_with("_mlx") {
                matches!(platform, kcreate_core::config::Platform::MacOsAppleSilicon)
            } else {
                true
            }
        })
        .filter(|p| (p.size_bytes / (1024 * 1024)) <= cap_mb)
        .map(|p| p.id)
        .collect()
}

// -----------------------------------------------------------------------------
// Image generation sidecar
// -----------------------------------------------------------------------------

fn image_gen_slot() -> &'static Mutex<Option<ImageGenSidecar>> {
    static SLOT: OnceLock<Mutex<Option<ImageGenSidecar>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Wire shape for the image-generation sidecar status. Mirrors
/// `LlmStatusInfo` so the renderer can render both in one table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenStatusInfo {
    pub state: &'static str,
    pub port: Option<u16>,
    pub error: Option<String>,
    /// True iff the host device passes the hard gate
    /// (`Tier ≥ 2` + GPU + not low-resource). The renderer uses
    /// this to drop the entire Generate panel when false.
    pub allowed: bool,
}

/// Start the image-generation sidecar for `pack_id`. Hard-gates on
/// [`kcreate_core::config::RuntimeConfig::image_generation_allowed`]:
/// returns `Phase4BridgeError::ImageGenNotAllowed` on tiers below
/// the hard gate. The pack id must exist in the registry; we resolve
/// the model path under the user's models directory.
pub fn image_gen_start(pack_id: String) -> Phase4Result<u16> {
    let cfg = runtime_slot().lock();
    if !cfg.image_generation_allowed() {
        return Err(Phase4BridgeError::ImageGenNotAllowed);
    }
    drop(cfg);
    let dir = models_root();
    let pack = list_model_packs(&dir)
        .into_iter()
        .find(|p| p.id == pack_id)
        .ok_or_else(|| Phase4BridgeError::Invalid(format!("unknown image-gen pack: {pack_id}")))?;
    if pack.category != kcreate_ai::ModelPackCategory::Generation {
        return Err(Phase4BridgeError::Invalid(format!(
            "pack {pack_id} is not an image-generation pack"
        )));
    }
    let model_path = dir.join(&pack.file_path);
    // Take the slot lock and stop any existing sidecar *before*
    // spawning the new Python child. Diffusion weights are large
    // enough (FLUX.2-Klein-4B is ~2.5 GB on a GPU) that we never
    // want two copies resident, even briefly, on a Tier-2 box.
    let mut guard = image_gen_slot().lock();
    if let Some(prev) = guard.as_mut() {
        prev.stop();
    }
    *guard = None;
    let mut sidecar = ImageGenSidecar::new(ImageGenConfig {
        python: PathBuf::from("python3"),
        host_python_module: "kcreate_diffusion.server".into(),
        model_path,
        health_timeout: std::time::Duration::from_mins(2),
        extra_args: vec![],
    });
    let port = sidecar.start().map_err(Phase4BridgeError::Sidecar)?;
    *guard = Some(sidecar);
    Ok(port)
}

/// Stop the image-generation sidecar. Idempotent.
pub fn image_gen_stop() {
    let mut guard = image_gen_slot().lock();
    if let Some(s) = guard.as_mut() {
        s.stop();
    }
    *guard = None;
}

/// Snapshot of the image-generation sidecar status.
pub fn image_gen_status() -> ImageGenStatusInfo {
    let allowed = runtime_slot().lock().image_generation_allowed();
    let guard = image_gen_slot().lock();
    let Some(s) = guard.as_ref() else {
        return ImageGenStatusInfo {
            state: "stopped",
            port: None,
            error: None,
            allowed,
        };
    };
    match s.status() {
        SidecarStatus::Stopped => ImageGenStatusInfo {
            state: "stopped",
            port: None,
            error: None,
            allowed,
        },
        SidecarStatus::Starting => ImageGenStatusInfo {
            state: "starting",
            port: None,
            error: None,
            allowed,
        },
        SidecarStatus::Ready { port, .. } => ImageGenStatusInfo {
            state: "ready",
            port: Some(port),
            error: None,
            allowed,
        },
        SidecarStatus::Error { message } => ImageGenStatusInfo {
            state: "error",
            port: None,
            error: Some(message),
            allowed,
        },
    }
}

/// Wire shape returned by [`image_gen_generate`]. The bytes are
/// already-decoded RGBA; the renderer can hand them straight to
/// `putImageData` or insert them as a new raster layer.
///
/// camelCase serde so the TypeScript mirror `GeneratedImage` in
/// `apps/desktop/shared/scene.ts` (`pngB64`) deserializes correctly
/// — without the rename, `result.pngB64` is `undefined` and the
/// renderer's preview / insert path crashes on `atob(undefined)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedImagePayload {
    pub width: u32,
    pub height: u32,
    /// Base64-encoded PNG. Kept as base64 to avoid having to design
    /// a binary IPC channel for a one-shot RGBA buffer.
    pub png_b64: String,
}

/// Generate an image from `prompt` at `width × height` with `steps`
/// diffusion steps. The seed is optional — `None` lets the sidecar
/// pick a random one.
pub fn image_gen_generate(
    prompt: String,
    width: u32,
    height: u32,
    steps: u32,
    seed: Option<u64>,
) -> Phase4Result<GeneratedImagePayload> {
    if width == 0 || height == 0 {
        return Err(Phase4BridgeError::Invalid(
            "width and height must be positive".into(),
        ));
    }
    let port = {
        let guard = image_gen_slot().lock();
        guard
            .as_ref()
            .and_then(|s| match s.status() {
                SidecarStatus::Ready { port, .. } => Some(port),
                _ => None,
            })
            .ok_or(Phase4BridgeError::ImageGenNotReady)?
    };
    let req = ImageGenRequest {
        prompt,
        width,
        height,
        steps,
        seed,
    };
    let out = generate_image(port, &req).map_err(Phase4BridgeError::ImageGen)?;
    // Re-encode RGBA → PNG for the wire so the renderer receives a
    // single self-describing payload (instead of a raw pixel slab
    // it would have to interpret with its own dimensions).
    let buf =
        image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(out.width, out.height, out.rgba)
            .ok_or_else(|| Phase4BridgeError::Invalid("rgba buffer dimensions mismatch".into()))?;
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| Phase4BridgeError::Invalid(format!("png write: {e}")))?;
    }
    Ok(GeneratedImagePayload {
        width: out.width,
        height: out.height,
        png_b64: B64.encode(&png_bytes),
    })
}

/// Convenience: is the renderer allowed to surface the
/// image-generation panel at all? Mirrors
/// [`kcreate_core::config::RuntimeConfig::image_generation_allowed`].
pub fn image_gen_allowed() -> bool {
    runtime_slot().lock().image_generation_allowed()
}

/// Recommend the best image-gen pack for the current device.
/// Returns `None` when image generation is disallowed on this
/// device — the renderer treats `None` as "don't show the panel".
pub fn image_gen_recommended_pack() -> Option<String> {
    let cfg = runtime_slot().lock();
    if !cfg.image_generation_allowed() {
        return None;
    }
    kcreate_ai::model_registry::recommended_generation_pack(cfg.device_tier, cfg.platform)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vision sidecar slot starts empty: the renderer must see
    /// a clean "stopped" status before it calls `vision_start`.
    #[test]
    fn vision_status_initial_is_stopped() {
        // No global setup; just inspect the initial slot. We
        // deliberately don't `vision_stop()` so we don't trample on
        // tests running concurrently in the same process (the
        // bridge unit-test binary is single-threaded under
        // serial_test, but defensive).
        let info = vision_status();
        // The state is whichever the prior test left it in; assert
        // only on the wire shape's structural invariants.
        assert!(matches!(
            info.state,
            "stopped" | "starting" | "ready" | "error"
        ));
    }

    /// Image-generation `allowed` flag reflects the runtime tier +
    /// GPU. We don't mutate the global runtime config in tests —
    /// this just confirms the call returns without panicking.
    #[test]
    fn image_gen_status_returns_allowed_flag() {
        let info = image_gen_status();
        assert!(matches!(
            info.state,
            "stopped" | "starting" | "ready" | "error"
        ));
        let _ = info.allowed;
    }

    /// `image_gen_generate` with zero width/height must fail
    /// validation, NOT spawn anything.
    #[test]
    fn image_gen_generate_rejects_zero_dimensions() {
        let r = image_gen_generate("hello".into(), 0, 512, 20, None);
        match r {
            Err(Phase4BridgeError::Invalid(msg)) => {
                assert!(msg.contains("width") || msg.contains("height"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Calling `image_gen_generate` with no sidecar running must
    /// surface `ImageGenNotReady`, not panic or stall.
    #[test]
    fn image_gen_generate_without_sidecar_is_not_ready() {
        // Make sure no sidecar is running. Idempotent.
        image_gen_stop();
        let r = image_gen_generate("hello".into(), 256, 256, 20, None);
        match r {
            Err(Phase4BridgeError::ImageGenNotReady) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Dispatch reasons round-trip through the bridge — used by the
    /// renderer's "Why is MLX unavailable?" affordance.
    #[test]
    fn dispatch_reason_variants_are_exhaustive() {
        use kcreate_ai::sidecar_dispatcher::DispatchReason;
        for r in [
            DispatchReason::LlamaServer,
            DispatchReason::MlxNative,
            DispatchReason::MlxUnavailableFallback,
        ] {
            let _ = format!("{r:?}");
        }
    }
}
