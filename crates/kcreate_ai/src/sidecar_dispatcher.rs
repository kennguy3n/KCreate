//! Unified sidecar dispatcher.
//!
//! Phase 12 Block A consolidated the AI stack on `llama-server`
//! across every supported platform — the original MLX (Apple
//! Silicon) sidecar that wrapped `python3 -m mlx_lm.server` has
//! been deleted along with the Python runtime dependency it
//! pulled in. Every text and vision pack now loads through
//! [`crate::llm_sidecar::LlmSidecar`], which speaks the same
//! OpenAI-compatible HTTP wire format on loopback that the chat
//! client (`llm_chat`) consumes.
//!
//! [`SidecarHandle`] is kept as a single-variant enum on purpose:
//! a future Rust-native inference engine (the candidate is a port
//! of llama.cpp's compute graph to `wgpu`) would land as a new
//! variant here without disturbing the bridge, so the enum is
//! the seam between dispatch policy and lifecycle wiring even
//! though it currently has only one arm.

use std::path::{Path, PathBuf};

use kcreate_core::config::Platform;

use crate::llm_sidecar::{LlmSidecar, SidecarConfig, SidecarError, SidecarResult, SidecarStatus};
use crate::model_registry::{list_model_packs, mmproj_for};

/// Which sidecar runtime is currently active. The chat client only
/// needs the port (which it queries through [`SidecarHandle::port`]);
/// this enum exists so the bridge can stop / poll the active
/// variant without knowing which one is loaded.
#[derive(Debug)]
pub enum SidecarHandle {
    /// llama-server with optional mmproj for vision.
    Llama(LlmSidecar),
}

impl SidecarHandle {
    /// Status snapshot.
    #[must_use]
    pub fn status(&self) -> SidecarStatus {
        match self {
            Self::Llama(s) => s.status(),
        }
    }

    /// Returns true iff the sidecar is in the Ready state.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status().is_ready()
    }

    /// Listening port (when ready). Returns `None` otherwise.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        match self.status() {
            SidecarStatus::Ready { port, .. } => Some(port),
            _ => None,
        }
    }

    /// Stop the underlying sidecar.
    pub fn stop(&mut self) {
        match self {
            Self::Llama(s) => s.stop(),
        }
    }

    /// Which runtime is active.
    #[must_use]
    pub fn runtime(&self) -> SidecarRuntime {
        match self {
            Self::Llama(_) => SidecarRuntime::LlamaServer,
        }
    }
}

/// Tag identifying which runtime backs a [`SidecarHandle`].
///
/// Kept as a single-variant enum (rather than collapsing to a unit
/// constant) so that adding a future Rust-native inference engine
/// — or re-introducing a per-platform fast path — is a non-breaking
/// addition. Bridge code matches on this enum exhaustively so the
/// compiler will flag every callsite the day a second variant lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarRuntime {
    LlamaServer,
}

/// The decision the dispatcher makes for a given pack id + platform.
/// Public so callers (and tests) can see *why* a particular runtime
/// was chosen — useful for the model-manager UI when explaining
/// which sidecar backs a given pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchPlan {
    pub runtime: SidecarRuntime,
    /// Resolved on-disk weights path.
    pub model_path: PathBuf,
    /// Resolved mmproj path (only meaningful for llama-server vision
    /// packs).
    pub mmproj_path: Option<PathBuf>,
    /// The pack id that was selected. Phase 12 dispatcher always
    /// returns the requested pack id unchanged because there is no
    /// longer a fallback table; an unknown pack is a hard error.
    pub resolved_pack_id: String,
    /// Why the dispatcher chose this runtime.
    pub reason: DispatchReason,
}

/// Why the dispatcher chose a particular runtime.
///
/// Single-variant for the same reason as [`SidecarRuntime`] — keeps
/// the bridge / UI matching exhaustively while leaving room for
/// future runtimes to add their own selection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchReason {
    /// Pack is a GGUF and the dispatcher will spawn llama-server.
    LlamaServer,
}

/// Build a dispatch plan WITHOUT spawning anything. Pure function so
/// tests can exercise the selection logic on any host.
///
/// `models_dir` resolves pack file paths. The `_platform` argument
/// is retained for forward compatibility (a future per-OS
/// optimisation may want to branch on it) but is currently unused —
/// Phase 12 Block A removed the MLX branch, so the dispatcher takes
/// the same code path on every supported host.
pub fn plan_dispatch(
    pack_id: &str,
    models_dir: &Path,
    _platform: Platform,
) -> SidecarResult<DispatchPlan> {
    let packs = list_model_packs(models_dir);

    let pack = packs
        .iter()
        .find(|p| p.id == pack_id)
        .ok_or_else(|| SidecarError::ModelMissing(PathBuf::from(pack_id)))?;
    if pack.file_path.is_empty() {
        return Err(SidecarError::ModelMissing(PathBuf::from(pack_id)));
    }
    let model_path = models_dir.join(&pack.file_path);

    // mmproj resolution must be all-or-nothing: if the registry says a
    // vision pack needs a projector companion (`mmproj_for` returns
    // `Some(...)`) but the companion isn't in `static_packs()`, the
    // previous `and_then(...).map(...)` chain silently dropped the
    // mmproj path and llama-server would start *without* `--mmproj` —
    // degrading to text-only inference and producing a baffling
    // "vision model returns gibberish on images" bug downstream
    // instead of a loud error here. The `mmproj_for_targets_resolve_to_real_packs`
    // test in `model_registry.rs` already pins this invariant on
    // every static pack, but treat a registry edit that breaks the
    // invariant as a hard error at dispatch time too, so the failure
    // surfaces at `vision_start` instead of as a quality regression
    // hours later.
    let mmproj_path = match mmproj_for(pack_id) {
        None => None,
        Some(m_id) => {
            let companion = packs.iter().find(|p| p.id == m_id).ok_or_else(|| {
                SidecarError::ModelMissing(PathBuf::from(format!(
                    "{m_id} (mmproj companion declared by registry for `{pack_id}` \
                     but missing from `static_packs()`)"
                )))
            })?;
            Some(models_dir.join(&companion.file_path))
        }
    };

    Ok(DispatchPlan {
        runtime: SidecarRuntime::LlamaServer,
        model_path,
        mmproj_path,
        resolved_pack_id: pack_id.to_string(),
        reason: DispatchReason::LlamaServer,
    })
}

/// Spawn the appropriate sidecar for `pack_id` and return a
/// [`SidecarHandle`]. The handle starts in `Starting`, transitions
/// to `Ready` once the child binds and responds to `/health`.
///
/// `models_dir` is the user's local models directory; the function
/// never reaches over the network.
pub fn start_for_pack(
    pack_id: &str,
    models_dir: &Path,
    platform: Platform,
) -> SidecarResult<SidecarHandle> {
    let plan = plan_dispatch(pack_id, models_dir, platform)?;
    start_with_plan(&plan)
}

/// Spawn a sidecar from an explicit [`DispatchPlan`]. Used by
/// `start_for_pack` and by tests that want to drive the dispatcher
/// without invoking platform probes.
pub fn start_with_plan(plan: &DispatchPlan) -> SidecarResult<SidecarHandle> {
    match plan.runtime {
        SidecarRuntime::LlamaServer => {
            let mut cfg = SidecarConfig::new(plan.model_path.clone());
            if let Some(mmproj) = plan.mmproj_path.as_ref() {
                cfg = cfg.with_mmproj(Some(mmproj.clone()));
            }
            let mut s = LlmSidecar::new(cfg);
            s.start()?;
            Ok(SidecarHandle::Llama(s))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `plan_dispatch` for a plain GGUF pack must always choose
    /// llama-server, regardless of platform.
    #[test]
    fn plain_gguf_pack_uses_llama_server() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_dispatch("llm_sidecar_3b", dir.path(), Platform::LinuxX64).unwrap();
        assert_eq!(plan.runtime, SidecarRuntime::LlamaServer);
        assert_eq!(plan.reason, DispatchReason::LlamaServer);
        assert!(plan.mmproj_path.is_none());
        assert_eq!(plan.resolved_pack_id, "llm_sidecar_3b");
    }

    /// Phase 12 Block A: the Ternary-Bonsai packs land on
    /// llama-server with no mmproj companion, on every platform.
    #[test]
    fn bonsai_packs_dispatch_to_llama_server() {
        let dir = tempfile::tempdir().unwrap();
        for pack_id in ["llm_bonsai_1_7b", "llm_bonsai_4b", "llm_bonsai_8b"] {
            for platform in [
                Platform::LinuxX64,
                Platform::WindowsX64,
                Platform::MacOsIntel,
                Platform::MacOsAppleSilicon,
            ] {
                let plan = plan_dispatch(pack_id, dir.path(), platform).unwrap();
                assert_eq!(plan.runtime, SidecarRuntime::LlamaServer);
                assert_eq!(plan.reason, DispatchReason::LlamaServer);
                assert!(plan.mmproj_path.is_none());
                assert_eq!(plan.resolved_pack_id, pack_id);
            }
        }
    }

    /// Unknown pack ids surface as `ModelMissing` rather than
    /// spawning anything. Phase 12 removed the MLX fallback table,
    /// so requesting a no-longer-shipped MLX pack id (which a stale
    /// project file could legitimately do) is also a hard error.
    #[test]
    fn unknown_pack_id_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = plan_dispatch("does_not_exist", dir.path(), Platform::LinuxX64).unwrap_err();
        assert!(matches!(err, SidecarError::ModelMissing(_)));

        // Legacy MLX pack ids — they used to fall back via
        // `gguf_fallback_for_mlx_pack`, but Phase 12 dropped that
        // table along with the MLX runtime, so the dispatcher now
        // reports them as missing instead of silently rerouting at
        // the dispatch layer. Migration of stale ids is handled at
        // the bridge entry point (`kcreate_bridge::phase4::
        // resolve_pack_id`) via the
        // `kcreate_ai::model_registry::migrate_legacy_pack_id`
        // table — so the user-facing UX path stays smooth, but the
        // dispatcher's contract is unchanged: it sees a current id
        // or surfaces `ModelMissing`. Keep this assertion in place
        // so a future refactor that conflates the two layers shows
        // up as a test failure.
        for legacy_mlx in [
            "vision_smolvlm_256m_mlx",
            "vision_qwen25vl_7b_mlx",
            "image_gen_flux_klein_mlx",
        ] {
            let err =
                plan_dispatch(legacy_mlx, dir.path(), Platform::MacOsAppleSilicon).unwrap_err();
            assert!(
                matches!(err, SidecarError::ModelMissing(_)),
                "expected legacy MLX pack {legacy_mlx} to surface as missing, got {err:?}",
            );
            // The migration helper must still rewrite them — this
            // is the post-condition the bridge relies on. If a
            // future change ever drops the migration table, this
            // assertion catches it without needing a separate test.
            assert!(
                crate::model_registry::migrate_legacy_pack_id(legacy_mlx).is_some(),
                "Phase 12 migration table must still rewrite legacy id {legacy_mlx}",
            );
        }
    }

    /// Vision pack on llama-server must include the mmproj path.
    #[test]
    fn vision_pack_on_llama_server_resolves_mmproj() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_dispatch("vision_smolvlm2_256m", dir.path(), Platform::LinuxX64).unwrap();
        assert_eq!(plan.runtime, SidecarRuntime::LlamaServer);
        assert!(plan.mmproj_path.is_some());
        assert!(plan
            .mmproj_path
            .as_ref()
            .unwrap()
            .ends_with("smolvlm2-256m-mmproj-f16.gguf"));
    }

    /// Same vision pack on Apple Silicon — Phase 12 makes the
    /// dispatch path platform-agnostic, so the same plan must
    /// appear regardless of host.
    #[test]
    fn vision_pack_dispatch_is_platform_agnostic() {
        let dir = tempfile::tempdir().unwrap();
        let linux = plan_dispatch("vision_qwen25vl_7b", dir.path(), Platform::LinuxX64).unwrap();
        let mac = plan_dispatch(
            "vision_qwen25vl_7b",
            dir.path(),
            Platform::MacOsAppleSilicon,
        )
        .unwrap();
        assert_eq!(linux, mac);
        assert_eq!(linux.runtime, SidecarRuntime::LlamaServer);
        assert!(linux
            .mmproj_path
            .as_ref()
            .unwrap()
            .ends_with("mmproj-qwen2.5-vl-7b-instruct-f16.gguf"));
    }
}
