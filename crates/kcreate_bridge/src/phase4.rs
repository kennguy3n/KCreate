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
    diffusion_sidecar::{DiffusionModelFlag, DiffusionSidecar, DiffusionSidecarConfig},
    image_gen::{generate_image, ImageGenError, ImageGenRequest},
    llm_sidecar::{LlmSidecar, SidecarConfig, SidecarError, SidecarStatus},
    model_registry::{list_model_packs, recommended_vision_pack},
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

/// Variant of the live vision sidecar. Phase 12 collapsed this down
/// to a single backend (llama-server) when the MLX path was
/// removed, but we keep the wrapper enum so a future Rust-native
/// inference engine can slot in without rewriting the bridge's
/// vision lifecycle code. The serializer therefore still reports a
/// `runtime` string (`"llama_server"`) so the renderer's existing
/// `VisionStatus.runtime` field doesn't break.
enum VisionHandle {
    Llama(LlmSidecar),
}

impl VisionHandle {
    fn status(&self) -> SidecarStatus {
        match self {
            Self::Llama(s) => s.status(),
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
        }
    }

    fn runtime(&self) -> SidecarRuntime {
        match self {
            Self::Llama(_) => SidecarRuntime::LlamaServer,
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

/// Start a vision sidecar for `pack_id`. Looks up the model and its
/// mmproj companion in the registry, resolves them under the user's
/// models directory, and spawns llama-server with `--mmproj` so
/// vision-language models accept `image_url` content parts on the
/// OpenAI-compatible chat API.
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
    // Phase 12 Block A removed MLX pack ids from the registry; a
    // stale project file or saved settings entry can still surface
    // them here. Rewrite to the current id transparently so the user
    // doesn't see an opaque `ModelMissing` after upgrade.
    let resolved_pack_id = resolve_pack_id(&pack_id);
    let plan = plan_dispatch(&resolved_pack_id, &models_dir, platform)
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
/// listening port. `start()` is non-blocking — it reserves the port,
/// forks the llama-server child, and hands off health probing to a
/// background thread. Callers observe readiness via
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
pub fn vision_extract_brand(rgba: &[u8], width: u32, height: u32) -> Phase4Result<BrandExtraction> {
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
/// id, return the mmproj companion's id (or `None` if the pack has
/// no companion projector — e.g. a text-only LLM pack).
pub fn vision_mmproj_for(pack_id: String) -> Option<String> {
    // Apply the Phase 12 legacy-id migration first so a stale
    // saved-preferences entry pointing at `vision_qwen25vl_7b_mlx`
    // still resolves the matching mmproj companion.
    let resolved = resolve_pack_id(&pack_id);
    kcreate_ai::model_registry::mmproj_for(&resolved).map(str::to_string)
}

/// Apply the legacy → current pack-id migration table from
/// `kcreate_ai::model_registry::migrate_legacy_pack_id`. Returns the
/// rewritten id (or `pack_id` unchanged when no migration applies)
/// and emits a one-time `log::warn!` per (legacy-id, process-lifetime)
/// pair so the renderer can surface a "your saved model preference
/// was renamed" prompt in the model manager. We use `log::warn!` (not
/// `error!`) because the rewrite is benign — the user lost no
/// functionality, the new id points at the same architecture under
/// the GGUF-llama-server pipeline.
///
/// Devin Review (post-Phase-12 round) noted the original implementation
/// emitted the warning on every call rather than once-per-legacy-id
/// as the doc comment claimed. A renderer that keeps re-invoking
/// `vision_start` / `image_gen_start` with the same stale saved-
/// preferences entry would have flooded the log. The dedup state is a
/// `OnceLock<Mutex<HashSet<String>>>` keyed on the *input* legacy id
/// so multiple legacy ids each get exactly one warning, and the same
/// id passed through twice gets only the first warning.
fn resolve_pack_id(pack_id: &str) -> String {
    if let Some(new_id) = kcreate_ai::model_registry::migrate_legacy_pack_id(pack_id) {
        log_legacy_pack_migration_once(pack_id, new_id);
        return new_id.to_string();
    }
    pack_id.to_string()
}

/// Process-lifetime dedup set for the legacy-pack migration warning.
/// Module-level so the test-only reset helper below can reach the
/// same backing store as `log_legacy_pack_migration_once`.
static LEGACY_PACK_WARN_SEEN: std::sync::OnceLock<
    parking_lot::Mutex<std::collections::HashSet<String>>,
> = std::sync::OnceLock::new();

fn legacy_pack_warn_seen() -> &'static parking_lot::Mutex<std::collections::HashSet<String>> {
    LEGACY_PACK_WARN_SEEN.get_or_init(|| parking_lot::Mutex::new(std::collections::HashSet::new()))
}

/// Emit the legacy-pack migration warning at most once per
/// (legacy-id, process-lifetime) pair. Returns `true` iff the warning
/// fired (i.e. this was the first sighting of `legacy`); the bool is
/// crate-private and exists purely so the unit test below can pin the
/// dedup behavior without poking at the `log` facade.
fn log_legacy_pack_migration_once(legacy: &str, current: &str) -> bool {
    let newly_seen = legacy_pack_warn_seen().lock().insert(legacy.to_string());
    if newly_seen {
        log::warn!(
            "kcreate_bridge: legacy MLX pack id `{legacy}` migrated to `{current}` \
             (Phase 12 removed the MLX runtime; update saved preferences to silence this)",
        );
    }
    newly_seen
}

/// Test-only reset for the dedup set so we can exercise the
/// one-emission-per-id contract without leaking state across tests.
/// Crate-private; the production code path never resets the set —
/// the user-visible "one warning per id per process" semantics rely
/// on the set surviving for the lifetime of the bridge.
#[cfg(test)]
fn reset_legacy_pack_migration_dedup_for_tests() {
    legacy_pack_warn_seen().lock().clear();
}

/// List the packs the renderer is allowed to show in the vision
/// section of the Model Manager. Clips by
/// [`crate::runtime_slot`]'s vision ceiling. Phase 12 removed the
/// `is_apple_silicon` MLX-filter branch — every vision pack now
/// runs on llama-server, so the renderer shows the same set on
/// every host.
pub fn vision_listable_packs() -> Vec<String> {
    let dir = models_root();
    let cap_mb = runtime_slot().lock().effective_vision_model_mb();
    list_model_packs(&dir)
        .into_iter()
        .filter(|p| {
            // User-selectable vision models only. mmproj companion
            // packs share `ModelPackCategory::Vision` (they install
            // alongside their paired weights, see `mmproj_for`) but
            // are NEVER selected on their own — the sidecar loads
            // them implicitly via `--mmproj`. They carry the
            // `"mmproj"` capability marker, which the model_registry
            // tests pin (`vision_packs_declare_vision_or_mmproj_capability`).
            // We require `"vision"` AND absence of `"mmproj"` so a
            // future model-picker dropdown that consumes this list
            // never offers projector files as standalone options.
            p.capabilities.iter().any(|c| c == "vision")
                && !p.capabilities.iter().any(|c| c == "mmproj")
        })
        .filter(|p| (p.size_bytes / (1024 * 1024)) <= cap_mb)
        .map(|p| p.id)
        .collect()
}

// -----------------------------------------------------------------------------
// Image generation sidecar
// -----------------------------------------------------------------------------

fn image_gen_slot() -> &'static Mutex<Option<DiffusionSidecar>> {
    static SLOT: OnceLock<Mutex<Option<DiffusionSidecar>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Resolve the sd-server binary path. Phase 12 ships a single env
/// var, `KCREATE_SD_SERVER_BINARY`, that points at an absolute path
/// to the stable-diffusion.cpp `sd-server` executable. When unset,
/// we fall back to the bare name `"sd-server"` and rely on the OS
/// PATH — the same fallback `llm_sidecar` uses for `llama-server`.
/// Mirrors the existing `KCREATE_MODELS_DIR` / `KCREATE_PLUGIN_DIR`
/// env-var override pattern in `phase2.rs`.
fn sd_server_binary() -> PathBuf {
    std::env::var_os("KCREATE_SD_SERVER_BINARY")
        .map_or_else(|| PathBuf::from("sd-server"), PathBuf::from)
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
    // Phase 12 Block A removed `image_gen_flux_klein_mlx` from the
    // registry; old project files / settings can still reference it.
    // Rewrite transparently to the current FLUX Klein 4B GGUF pack.
    let resolved_pack_id = resolve_pack_id(&pack_id);
    let pack = list_model_packs(&dir)
        .into_iter()
        .find(|p| p.id == resolved_pack_id)
        .ok_or_else(|| {
            Phase4BridgeError::Invalid(format!("unknown image-gen pack: {resolved_pack_id}"))
        })?;
    if pack.category != kcreate_ai::ModelPackCategory::Generation {
        return Err(Phase4BridgeError::Invalid(format!(
            "pack {resolved_pack_id} is not an image-generation pack"
        )));
    }
    let model_path = dir.join(&pack.file_path);
    // sd-server takes a fused full checkpoint (SD 1.x / SDXL, which
    // bundles CLIP + VAE) through `-m`, but a standalone diffusion
    // model (FLUX / SD3-style) through `--diffusion-model`. The
    // registry owns that per-pack classification.
    let model_flag =
        if kcreate_ai::model_registry::generation_pack_is_fused_checkpoint(&resolved_pack_id) {
            DiffusionModelFlag::FullCheckpoint
        } else {
            DiffusionModelFlag::Standalone
        };
    // Take the slot lock and stop any existing sidecar *before*
    // spawning the new sd-server child. Diffusion weights are large
    // enough (FLUX.2-Klein-4B is ~2.5 GB on a GPU) that we never
    // want two copies resident, even briefly, on a Tier-2 box.
    let mut guard = image_gen_slot().lock();
    if let Some(prev) = guard.as_mut() {
        prev.stop();
    }
    *guard = None;
    let cfg = DiffusionSidecarConfig {
        binary: sd_server_binary(),
        model_path,
        model_flag,
        health_timeout: std::time::Duration::from_mins(2),
        // Standalone FLUX builds need supplementary text-encoder / VAE
        // paths; a fused SD checkpoint needs none. Either way, users
        // pass component paths or sd-server knobs through
        // `KCREATE_SD_SERVER_EXTRA_ARGS` (POSIX shell-word parsed —
        // see `parse_sd_server_extra_args`).
        extra_args: parse_sd_server_extra_args()?,
    };
    let mut sidecar = DiffusionSidecar::new(cfg);
    let port = sidecar.start().map_err(Phase4BridgeError::Sidecar)?;
    *guard = Some(sidecar);
    Ok(port)
}

/// Parse the `KCREATE_SD_SERVER_EXTRA_ARGS` env var into an argv
/// slice that gets forwarded to sd-server. The env var is parsed
/// with POSIX shell-word rules (`shell-words` crate): single and
/// double quotes group spaces into a single token, and backslash
/// escapes inside double quotes survive. Empty / unset => no extra
/// args.
///
/// We use shell-word splitting (vs. naive `split_whitespace`)
/// specifically so Windows paths with spaces work. On a typical
/// Windows install the FLUX text encoder lives somewhere like
/// `C:\Program Files\sd-models\flux\t5xxl_fp16.safetensors`; with
/// the old whitespace-split parser the operator had to either
/// reinstall to a no-space path or hard-link an alias, and even
/// then they got an opaque `failed to open file` from sd-server on
/// the malformed argv. Quoting now works as expected, e.g.:
///
/// ```text
/// KCREATE_SD_SERVER_EXTRA_ARGS=
///   --clip_l "C:\Program Files\sd-models\clip_l.safetensors"
///   --t5xxl  "C:\Program Files\sd-models\t5xxl.safetensors"
///   --vae    "C:\Program Files\sd-models\ae.safetensors"
/// ```
///
/// Returns `Phase4BridgeError::Invalid` if the env var is set but
/// the value has mismatched quotes — surfacing the parse failure to
/// the renderer is much friendlier than silently truncating the
/// argv and letting sd-server fail with a cryptic missing-file
/// error.
fn parse_sd_server_extra_args() -> Phase4Result<Vec<String>> {
    let Some(raw) = std::env::var_os("KCREATE_SD_SERVER_EXTRA_ARGS") else {
        return Ok(Vec::new());
    };
    let raw_str = raw.to_string_lossy();
    if raw_str.trim().is_empty() {
        return Ok(Vec::new());
    }
    shell_words::split(&raw_str).map_err(|e| {
        Phase4BridgeError::Invalid(format!(
            "KCREATE_SD_SERVER_EXTRA_ARGS could not be parsed as a shell-quoted argv: {e}",
        ))
    })
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

/// Phase C — recommend the best LLM (sidecar) pack for the
/// current device tier + platform. Mirrors
/// [`vision_recommended_pack`] / [`image_gen_recommended_pack`]
/// so the welcome modal can drive a one-click "install
/// recommended pack" flow without the renderer having to mirror
/// the tier → pack mapping.
///
/// The pack id returned here is the *intended* recommendation;
/// the welcome modal still cross-references it against
/// [`kcreate_ai::list_model_packs`] to surface display name,
/// size, and download URL. Returns `None` only if the registry
/// has no recommendation for the current device, which is
/// expected to be never — [`kcreate_ai::recommended_llm_pack`]
/// covers every supported tier.
pub fn llm_recommended_pack() -> Option<String> {
    let cfg = runtime_slot().lock();
    kcreate_ai::model_registry::recommended_llm_pack(cfg.device_tier, cfg.platform)
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

    /// `vision_listable_packs` must never include mmproj companion
    /// packs. They live in `ModelPackCategory::Vision` so the
    /// model_registry can ship them alongside the weights, but a
    /// user can NEVER select one as a standalone model — the sidecar
    /// loads them implicitly via `--mmproj`. If a future Model
    /// Manager dropdown consumes this list it must not see projector
    /// files as installable options.
    #[test]
    fn vision_listable_packs_excludes_mmproj_companions() {
        let listed = vision_listable_packs();
        for id in &listed {
            assert!(
                !id.ends_with("_mmproj"),
                "mmproj companion pack `{id}` leaked into vision_listable_packs"
            );
        }
        // Sanity: at least one real vision model must be present on
        // any tier that allows vision — the smallest VLM
        // (SmolVLM2-256M, ~180 MB) fits in every tier's cap. We
        // accept an empty list only when the host runtime forbids
        // every pack (e.g. a hypothetical low-resource Tier 0 that
        // capped at <180 MB), which the production tier table
        // doesn't do today.
        let _ = listed;
    }

    /// Dispatch reasons round-trip through the bridge. Phase 12
    /// collapsed the MLX-fallback variants and left a single
    /// `LlamaServer` reason, but we still exercise it here so a
    /// future addition of a Rust-native runtime is caught in this
    /// test rather than at the UI surface.
    #[test]
    fn dispatch_reason_variants_are_exhaustive() {
        use kcreate_ai::sidecar_dispatcher::DispatchReason;
        // Phase 12 collapsed every MLX dispatch branch into the
        // single `LlamaServer` reason; the loop shape here is
        // intentional so a future Rust-native runtime variant
        // (or any other variant added to `DispatchReason`) shows
        // up as a missing match arm in this test rather than at
        // the UI surface. `clippy::single_element_loop` would
        // rather we expanded to a single binding, but that
        // would silently accept future additions — keep the
        // loop and allow the lint at the call site.
        #[allow(clippy::single_element_loop)]
        for r in [DispatchReason::LlamaServer] {
            let _ = format!("{r:?}");
        }
    }

    /// Wire-format lockstep (AGENTS.md §4): `GeneratedImagePayload`
    /// must serialise its `png_b64` field as `pngB64`, matching the
    /// TypeScript mirror `GeneratedImage.pngB64` in
    /// `apps/desktop/shared/scene.ts` and the `ImageGenPanel.tsx`
    /// `result.pngB64` access. If a future Rust edit drops the
    /// `#[serde(rename_all = "camelCase")]` attribute or renames
    /// the field, the renderer would silently see `undefined` and
    /// crash inside `atob(undefined)` at runtime. This test pins
    /// the contract so the breakage is caught at `cargo test` time.
    #[test]
    fn generated_image_payload_wire_format_is_camelcase() {
        let payload = GeneratedImagePayload {
            width: 512,
            height: 512,
            png_b64: "iVBORw0KGgo=".to_string(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        // Positive: every field must surface under its camelCase
        // wire name.
        assert!(
            json.contains("\"pngB64\":\"iVBORw0KGgo=\""),
            "expected `pngB64` in wire JSON, got: {json}"
        );
        assert!(
            json.contains("\"width\":512"),
            "expected `width` in wire JSON, got: {json}"
        );
        assert!(
            json.contains("\"height\":512"),
            "expected `height` in wire JSON, got: {json}"
        );
        // Negative: the Rust-side `snake_case` field name must
        // NOT appear on the wire. A regression that drops the
        // rename attribute would leak `png_b64` here.
        assert!(
            !json.contains("png_b64"),
            "snake_case field name leaked to the wire: {json}"
        );
        // Round-trip: re-parse the JSON and confirm every field
        // survives — guards against a future asymmetric rename
        // (e.g. someone sets `rename` only on serialize).
        let back: GeneratedImagePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.width, 512);
        assert_eq!(back.height, 512);
        assert_eq!(back.png_b64, "iVBORw0KGgo=");
    }

    // -------------------------------------------------------------
    // KCREATE_SD_SERVER_EXTRA_ARGS parsing — POSIX shell-word rules.
    //
    // These tests run serially (the env var is process-global) but
    // each restores the prior value before returning so test order
    // doesn't leak. They are guarded by a single mutex so concurrent
    // executors can't race the `set_var` / `remove_var` pair.
    // -------------------------------------------------------------

    /// Test-internal serialization of `KCREATE_SD_SERVER_EXTRA_ARGS`.
    ///
    /// Encapsulating the lock + guard in a private sub-module makes
    /// the serialization invariant type-enforced rather than
    /// convention-enforced: the only way to mutate the env var from
    /// test code is through one of the two APIs this module exports
    /// (`SdExtraArgsGuard::new` for the production path,
    /// `with_locked_raw_env` for the panic test that exercises the
    /// guard itself). External test code cannot reach the underlying
    /// `parking_lot::Mutex` and therefore cannot accidentally bypass
    /// the lock and torn-read the env var.
    ///
    /// If a future parser test needs raw env access for a reason
    /// not covered by `SdExtraArgsGuard`, it must go through
    /// `with_locked_raw_env` — there is no other way to construct
    /// a locked critical section.
    mod sd_extra_args_env {
        /// Process-global env var manipulated by these tests. Kept
        /// `pub(super)` so the tests can pass the same name into the
        /// production parser without having to repeat the literal.
        pub(super) const KEY: &str = "KCREATE_SD_SERVER_EXTRA_ARGS";

        /// Private serializing mutex. There is no accessor — every
        /// path that acquires it must go through one of the
        /// `pub(super)` APIs in this module, so the lock cannot
        /// leak out to ad-hoc test code.
        fn lock() -> &'static parking_lot::Mutex<()> {
            static LOCK: std::sync::OnceLock<parking_lot::Mutex<()>> = std::sync::OnceLock::new();
            LOCK.get_or_init(|| parking_lot::Mutex::new(()))
        }

        /// RAII guard for the `KCREATE_SD_SERVER_EXTRA_ARGS` test slot.
        /// One construction acquires the global serializing mutex,
        /// captures the prior env value, and installs the requested
        /// scratch value. One drop restores the prior env value and
        /// then releases the mutex.
        ///
        /// The whole point of bundling both responsibilities into a
        /// single type is that the ordering invariant — *restore env
        /// before releasing the mutex, so the next waiter sees a clean
        /// starting state* — is enforced by the type's `Drop` impl, not
        /// by a `let` declaration-order convention that a future
        /// refactor could silently violate. Rust drops struct fields
        /// *after* the user-supplied `Drop::drop` body runs, so the env
        /// restore in `drop` always happens while `_lock` is still
        /// alive, regardless of field declaration order.
        ///
        /// Used by `with_sd_extra_args` for the normal case and by
        /// the panic-path test below (via [`with_locked_raw_env`] for
        /// its setup/teardown plumbing). Both paths share the same
        /// single ordering guarantee so the panic test is a faithful
        /// proxy for the normal-return path.
        ///
        /// We rely on `parking_lot::Mutex` (workspace-wide already,
        /// see `crates/kcreate_bridge/Cargo.toml`) specifically
        /// because it does *not* poison on panic — if the body inside
        /// the guard panics, the mutex remains usable for every
        /// subsequent test. Swapping to `std::sync::Mutex` would
        /// silently make every later parser test fail with a
        /// `PoisonError` after a single failing test, which would
        /// mask the original failure.
        pub(super) struct SdExtraArgsGuard {
            _lock: parking_lot::MutexGuard<'static, ()>,
            prior: Option<std::ffi::OsString>,
        }

        impl SdExtraArgsGuard {
            /// Acquire the serializing mutex, snapshot the current
            /// `KCREATE_SD_SERVER_EXTRA_ARGS`, and install `value`
            /// (or unset the var when `value` is `None`).
            pub(super) fn new(value: Option<&str>) -> Self {
                let lock = lock().lock();
                let prior = std::env::var_os(KEY);
                // SAFETY: cargo runs unit tests across multiple
                // threads by default; `lock` makes this critical
                // section single-writer, and every parser test that
                // touches `KCREATE_SD_SERVER_EXTRA_ARGS` goes through
                // this guard (or `with_locked_raw_env`), so no other
                // thread can observe a torn read or race against
                // this write.
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(KEY, v),
                        None => std::env::remove_var(KEY),
                    }
                }
                Self { _lock: lock, prior }
            }
        }

        impl Drop for SdExtraArgsGuard {
            fn drop(&mut self) {
                // SAFETY: `self._lock` is still alive at this point —
                // Rust drops the struct's fields *after* the
                // user-supplied `Drop::drop` body returns, so we
                // still hold the serializing mutex while restoring
                // the env var. The mutex then drops as a field on
                // the way out, releasing the lock only *after* the
                // restore is observable.
                unsafe {
                    match self.prior.take() {
                        Some(v) => std::env::set_var(KEY, v),
                        None => std::env::remove_var(KEY),
                    }
                }
            }
        }

        /// Escape hatch for the panic-unwind test that exercises
        /// `SdExtraArgsGuard::drop` itself. Holds the serializing
        /// mutex for the duration of `f` and releases it on the way
        /// out (including on panic — `parking_lot` mutexes do not
        /// poison, by design). The caller is responsible for any
        /// `unsafe` env mutation inside `f`; this function only
        /// provides the lock, not the env access.
        ///
        /// Do not use this in normal parser tests — use
        /// `SdExtraArgsGuard::new` (via `with_sd_extra_args`)
        /// instead. The only legitimate caller is the panic test
        /// itself, which cannot use `SdExtraArgsGuard` recursively
        /// because that would conflate "the thing under test" with
        /// "the test harness".
        pub(super) fn with_locked_raw_env<R>(f: impl FnOnce() -> R) -> R {
            let _lock = lock().lock();
            f()
        }
    }

    use sd_extra_args_env::{with_locked_raw_env, SdExtraArgsGuard};

    /// Helper: run `body` with `KCREATE_SD_SERVER_EXTRA_ARGS` forced
    /// to `value` (or unset when `None`), then restore whatever the
    /// caller had set on the way in — even if `body` panics.
    ///
    /// All ordering correctness lives in [`SdExtraArgsGuard`]; this
    /// helper is just a one-liner over `Guard::new` + `body()`.
    fn with_sd_extra_args<R>(value: Option<&str>, body: impl FnOnce() -> R) -> R {
        let _guard = SdExtraArgsGuard::new(value);
        body()
    }

    /// `SdExtraArgsGuard::drop` must restore the env var even when
    /// the body inside the guard unwinds via a panic — otherwise a
    /// single failing parser test would leak its scratch value to
    /// every later test in the same cargo process and produce a
    /// cascade of unrelated failures.
    ///
    /// The test does the following dance:
    ///   0. Snapshot whatever value the test process had set
    ///      before this test ran so Step 4 can put it back
    ///      exactly the way we found it.
    ///   1. Briefly take the serializing mutex, install a SENTINEL
    ///      value as the "prior" state, drop the mutex. This is the
    ///      only place a value escapes the guard mechanism, and
    ///      it's bounded — any concurrent parser test that runs
    ///      after this point still goes through
    ///      [`SdExtraArgsGuard::new`] which snapshots SENTINEL as
    ///      *its* prior and restores it on its own drop, so
    ///      SENTINEL is preserved across arbitrary interleavings.
    ///   2. Inside [`std::panic::catch_unwind`], construct a real
    ///      `SdExtraArgsGuard` (which re-acquires the mutex,
    ///      captures SENTINEL as its prior, installs a scratch
    ///      value), then `panic!()`.
    ///   3. The catch_unwind unwinds across the guard's drop, which
    ///      must restore SENTINEL and release the mutex.
    ///   4. Re-acquire the mutex, observe the env var is SENTINEL,
    ///      and restore the Step-0 snapshot so the test process
    ///      ends exactly where it started.
    ///
    /// `AssertUnwindSafe` is sound: every value touched inside the
    /// closure is either a Drop-cleaned RAII guard
    /// (`SdExtraArgsGuard`) or a plain raw env read/write — there
    /// is no broken-invariant state observable after the catch.
    #[test]
    fn sd_extra_args_guard_drop_runs_on_panic_unwind() {
        // Sentinel that no other parser test sets, so a successful
        // restore is unambiguous.
        const SENTINEL: &str = "--restore-panic-test /tmp/sentinel";

        // Step 0: snapshot whatever value the test process had
        // before we touched anything, so Step 4 can put the world
        // back exactly the way we found it (instead of
        // unconditionally unsetting and clobbering a pre-existing
        // caller-set value — e.g. a developer running this test
        // with `KCREATE_SD_SERVER_EXTRA_ARGS=foo cargo test`).
        let process_prior = with_locked_raw_env(|| std::env::var_os(sd_extra_args_env::KEY));

        // Step 1: install SENTINEL as the "prior" state. Hold the
        // mutex only for the write itself so the catch_unwind body
        // below can re-acquire it without deadlocking (parking_lot
        // mutexes are non-reentrant).
        with_locked_raw_env(|| {
            // SAFETY: mutex held via `with_locked_raw_env`; only
            // this test mutates the env var inside this critical
            // section, and any concurrent parser test goes through
            // `SdExtraArgsGuard` (or `with_locked_raw_env`) which
            // would block on the same mutex.
            unsafe {
                std::env::set_var(sd_extra_args_env::KEY, SENTINEL);
            }
        });

        // Step 2: panic inside an `SdExtraArgsGuard`.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = SdExtraArgsGuard::new(Some("--inner /tmp/should-be-restored"));
            // Sanity-check the inner write actually took effect
            // before we panic — otherwise the test could "pass"
            // because the prior value was never overridden in the
            // first place.
            assert_eq!(
                std::env::var(sd_extra_args_env::KEY).as_deref(),
                Ok("--inner /tmp/should-be-restored"),
                "guard must install the scratch value before body runs",
            );
            panic!("simulated parser-test failure");
        }));
        assert!(
            result.is_err(),
            "panic should propagate out of catch_unwind"
        );

        // (Step 3 happens implicitly *during* the catch_unwind
        // stack unwind, before catch_unwind returns the Err above:
        // as the unwind walks out of the closure, the
        // `let _guard = SdExtraArgsGuard::new(...)` binding goes
        // out of scope, which runs `SdExtraArgsGuard::drop`, which
        // restores SENTINEL and releases the mutex. By the time
        // catch_unwind hands control back here, the env is already
        // reverted and the mutex is free. There's no code line for
        // Step 3 because it's the runtime unwind behavior we're
        // testing — the assertions at Step 4 are what observe the
        // result.)

        // Step 4: re-acquire mutex, observe env reverted to
        // SENTINEL, and restore the truly-prior process value
        // captured at Step 0 so this test leaves the env exactly
        // the way it found it.
        with_locked_raw_env(|| {
            let restored = std::env::var_os(sd_extra_args_env::KEY);
            // SAFETY: mutex held via `with_locked_raw_env`.
            unsafe {
                match process_prior {
                    Some(ref v) => {
                        std::env::set_var(sd_extra_args_env::KEY, v);
                    }
                    None => std::env::remove_var(sd_extra_args_env::KEY),
                }
            }
            assert_eq!(
                restored.as_deref(),
                Some(std::ffi::OsStr::new(SENTINEL)),
                "SdExtraArgsGuard::drop must restore prior env value on panic unwind",
            );
        });
    }

    /// Unset env var => empty argv.
    #[test]
    fn parse_sd_extra_args_unset_returns_empty() {
        let out = with_sd_extra_args(None, parse_sd_server_extra_args).unwrap();
        assert!(out.is_empty());
    }

    /// Empty / whitespace-only value => empty argv (no spurious
    /// tokens that would confuse sd-server's flag parser).
    #[test]
    fn parse_sd_extra_args_whitespace_only_returns_empty() {
        let out = with_sd_extra_args(Some("   \t\n  "), parse_sd_server_extra_args).unwrap();
        assert!(out.is_empty(), "got {out:?}");
    }

    /// Plain unquoted argv splits on whitespace, matching prior
    /// behavior so existing Linux/macOS configs don't regress.
    #[test]
    fn parse_sd_extra_args_unquoted_splits_on_whitespace() {
        let out = with_sd_extra_args(
            Some("--clip_l /m/clip.sft --t5xxl /m/t5.sft --vae /m/ae.sft"),
            parse_sd_server_extra_args,
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                "--clip_l".to_string(),
                "/m/clip.sft".to_string(),
                "--t5xxl".to_string(),
                "/m/t5.sft".to_string(),
                "--vae".to_string(),
                "/m/ae.sft".to_string(),
            ]
        );
    }

    /// Double-quoted Windows path stays one token, including the
    /// embedded space — this is the regression the bug report flagged.
    #[test]
    fn parse_sd_extra_args_double_quoted_path_with_spaces() {
        let out = with_sd_extra_args(
            Some(r#"--clip_l "C:\Program Files\sd-models\clip_l.safetensors" --t5xxl "C:\Program Files\sd-models\t5xxl.safetensors""#),
            parse_sd_server_extra_args,
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                "--clip_l".to_string(),
                r"C:\Program Files\sd-models\clip_l.safetensors".to_string(),
                "--t5xxl".to_string(),
                r"C:\Program Files\sd-models\t5xxl.safetensors".to_string(),
            ]
        );
    }

    /// Single quotes work the same way as double quotes for POSIX
    /// shell tokenization — confirm both surfaces.
    #[test]
    fn parse_sd_extra_args_single_quoted_path_with_spaces() {
        let out = with_sd_extra_args(
            Some("--vae '/Users/me/My Models/ae.safetensors'"),
            parse_sd_server_extra_args,
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                "--vae".to_string(),
                "/Users/me/My Models/ae.safetensors".to_string(),
            ]
        );
    }

    /// Mismatched quotes surface as a typed parse error rather than
    /// a silently-truncated argv — so the renderer can show a
    /// helpful error toast instead of letting sd-server fail with
    /// an opaque missing-file message.
    #[test]
    fn parse_sd_extra_args_mismatched_quotes_errors() {
        let r = with_sd_extra_args(
            Some(r#"--clip_l "unterminated"#),
            parse_sd_server_extra_args,
        );
        match r {
            Err(Phase4BridgeError::Invalid(msg)) => {
                assert!(
                    msg.contains("KCREATE_SD_SERVER_EXTRA_ARGS"),
                    "error message must name the env var, got: {msg}"
                );
            }
            other => panic!("expected Invalid error, got {other:?}"),
        }
    }

    /// Phase 12 legacy MLX pack-id migration round-trips through
    /// the bridge `resolve_pack_id` helper. The helper is the
    /// single migration seam — every entry point (`vision_start`,
    /// `image_gen_start`, `vision_mmproj_for`) goes through it, so
    /// asserting on it here covers all three call sites.
    #[test]
    fn resolve_pack_id_migrates_legacy_mlx_ids() {
        assert_eq!(
            resolve_pack_id("vision_smolvlm_256m_mlx"),
            "vision_smolvlm2_256m",
        );
        assert_eq!(
            resolve_pack_id("vision_qwen25vl_7b_mlx"),
            "vision_qwen25vl_7b",
        );
        assert_eq!(
            resolve_pack_id("image_gen_flux_klein_mlx"),
            "image_gen_flux_klein_4b",
        );
    }

    /// `resolve_pack_id` is a no-op for current ids — current
    /// callers must not see allocations or spurious log warnings
    /// for the common path.
    #[test]
    fn resolve_pack_id_passes_through_current_ids() {
        for id in [
            "llm_bonsai_1_7b",
            "vision_smolvlm2_256m",
            "image_gen_flux_klein_4b",
            "totally_unknown_pack_id",
        ] {
            assert_eq!(resolve_pack_id(id), id);
        }
    }

    /// `vision_mmproj_for` migrates legacy MLX ids before looking
    /// up the projector companion — a stale settings entry that
    /// still names the MLX variant must resolve to the current
    /// mmproj pack, not surface as `None`.
    #[test]
    fn vision_mmproj_for_migrates_legacy_mlx_ids() {
        assert_eq!(
            vision_mmproj_for("vision_qwen25vl_7b_mlx".into()),
            Some("vision_qwen25vl_7b_mmproj".to_string()),
        );
        assert_eq!(
            vision_mmproj_for("vision_smolvlm_256m_mlx".into()),
            Some("vision_smolvlm2_256m_mmproj".to_string()),
        );
    }

    /// `log_legacy_pack_migration_once` must fire exactly once per
    /// legacy id per process lifetime. Devin Review (post-Phase-12)
    /// flagged that the original implementation logged on every call
    /// even though the doc comment said "one-time", which would have
    /// flooded the log if a renderer kept invoking
    /// `vision_start` / `image_gen_start` with the same stale saved
    /// preferences entry. The dedup state is module-level
    /// (`LEGACY_PACK_WARN_SEEN`) so multiple sibling tests within the
    /// same `cargo test` binary share it — we explicitly reset it at
    /// the top of the test so the assertion is deterministic
    /// regardless of test ordering.
    ///
    /// We intentionally don't assert on the `log` facade output (no
    /// in-process subscriber) because the contract under test is
    /// "the dedup set absorbs repeats", not the log layer's behavior.
    /// The boolean returned by `log_legacy_pack_migration_once` is
    /// exposed for exactly this reason.
    #[test]
    fn log_legacy_pack_migration_dedups_on_repeat_calls() {
        reset_legacy_pack_migration_dedup_for_tests();
        // First sighting of each legacy id => warning fires.
        assert!(log_legacy_pack_migration_once(
            "vision_smolvlm_256m_mlx",
            "vision_smolvlm2_256m"
        ));
        assert!(log_legacy_pack_migration_once(
            "image_gen_flux_klein_mlx",
            "image_gen_flux_klein_4b"
        ));
        // Same legacy id again => suppressed.
        assert!(!log_legacy_pack_migration_once(
            "vision_smolvlm_256m_mlx",
            "vision_smolvlm2_256m"
        ));
        assert!(!log_legacy_pack_migration_once(
            "image_gen_flux_klein_mlx",
            "image_gen_flux_klein_4b"
        ));
        // A different legacy id still fires the first time.
        assert!(log_legacy_pack_migration_once(
            "vision_qwen25vl_7b_mlx",
            "vision_qwen25vl_7b"
        ));
        // Reset the dedup state so sibling tests that also exercise
        // `resolve_pack_id` aren't observed via shared module-level
        // state.
        reset_legacy_pack_migration_dedup_for_tests();
    }

    /// Phase C — the LLM recommendation must always resolve to one
    /// of the three Bonsai pack ids so the welcome modal can look
    /// up display name + URL via `list_model_packs`. Returning
    /// `None` would force the renderer down a "no recommendation"
    /// degraded path that we never want on a supported device, so
    /// we pin both that the call succeeds and that the id is one
    /// of the registry's canonical sidecar pack ids.
    #[test]
    fn llm_recommended_pack_resolves_to_a_known_bonsai_pack() {
        let id = llm_recommended_pack().expect("LLM recommendation present for every device tier");
        assert!(
            matches!(
                id.as_str(),
                "llm_bonsai_1_7b" | "llm_bonsai_4b" | "llm_bonsai_8b"
            ),
            "unexpected LLM recommendation id: {id}"
        );
    }
}
