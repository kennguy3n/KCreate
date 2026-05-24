//! Unified sidecar dispatcher (Task 23).
//!
//! There are two sidecar runtimes in Phase 4:
//!
//! - [`crate::llm_sidecar::LlmSidecar`] — wraps `llama-server`,
//!   loads GGUF weights (`+ optional mmproj for vision`).
//! - [`crate::mlx_sidecar::MlxSidecar`] — wraps
//!   `python3 -m mlx_lm.server`, runs on Apple Silicon only, loads
//!   MLX-format models (single folder, or HF slug).
//!
//! Both expose the same OpenAI-compatible HTTP wire format on
//! loopback, so the chat client (`llm_chat`) does not care which
//! one is running. This module owns the *selection* logic:
//!
//! - If a pack id ends in `_mlx`, on Apple Silicon, AND
//!   [`crate::mlx_sidecar::probe_mlx_available`] is true ⇒ run MLX.
//! - Otherwise ⇒ run llama-server with the GGUF (plus its mmproj
//!   companion if the pack has one).
//!
//! The dispatcher itself holds the selected variant in an enum and
//! forwards `status`/`stop` calls so the bridge sees a single
//! lifecycle, not two parallel ones.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kcreate_core::config::Platform;

use crate::llm_sidecar::{LlmSidecar, SidecarConfig, SidecarError, SidecarResult, SidecarStatus};
use crate::mlx_sidecar::{probe_mlx_available, MlxSidecar, MlxSidecarConfig};
use crate::model_registry::{gguf_fallback_for_mlx_pack, list_model_packs, mmproj_for};

/// Which sidecar runtime is currently active. The chat client only
/// needs the port (which it queries through [`SidecarHandle::port`]);
/// this enum exists so the bridge can stop / poll the active
/// variant without knowing which one is loaded.
#[derive(Debug)]
pub enum SidecarHandle {
    /// llama-server with optional mmproj for vision.
    Llama(LlmSidecar),
    /// `python3 -m mlx_lm.server` on Apple Silicon.
    Mlx(MlxSidecar),
}

impl SidecarHandle {
    /// Status snapshot.
    #[must_use]
    pub fn status(&self) -> SidecarStatus {
        match self {
            Self::Llama(s) => s.status(),
            Self::Mlx(s) => s.status(),
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
            Self::Mlx(s) => s.stop(),
        }
    }

    /// Which runtime is active.
    #[must_use]
    pub fn runtime(&self) -> SidecarRuntime {
        match self {
            Self::Llama(_) => SidecarRuntime::LlamaServer,
            Self::Mlx(_) => SidecarRuntime::MlxLm,
        }
    }
}

/// Tag identifying which runtime backs a [`SidecarHandle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarRuntime {
    LlamaServer,
    MlxLm,
}

/// The decision the dispatcher makes for a given pack id + platform.
/// Public so callers (and tests) can see *why* a particular runtime
/// was chosen — useful for the model-manager UI's "Why is MLX not
/// available?" affordance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchPlan {
    pub runtime: SidecarRuntime,
    /// Resolved on-disk weights path.
    pub model_path: PathBuf,
    /// Resolved mmproj path (only meaningful for llama-server vision
    /// packs).
    pub mmproj_path: Option<PathBuf>,
    /// The pack id that was actually selected (after MLX
    /// fall-through logic). Useful when the user asked for an MLX
    /// pack but MLX wasn't available on the host.
    pub resolved_pack_id: String,
    /// Why the dispatcher chose this runtime.
    pub reason: DispatchReason,
}

/// Why the dispatcher chose a particular runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchReason {
    /// Pack id ends in `_mlx`, host is Apple Silicon, and MLX is
    /// installed — direct match.
    MlxNative,
    /// Pack id ends in `_mlx` but host isn't Apple Silicon or MLX
    /// isn't installed — fell back to the GGUF equivalent.
    MlxUnavailableFallback,
    /// Pack is a GGUF and the dispatcher will spawn llama-server.
    LlamaServer,
}

/// Build a dispatch plan WITHOUT spawning anything. Pure function so
/// tests can exercise the selection logic on any host.
///
/// `models_dir` resolves pack file paths; `mlx_available` is the
/// host's probed `mlx_lm` availability (the bridge caches this).
pub fn plan_dispatch(
    pack_id: &str,
    models_dir: &Path,
    platform: Platform,
    mlx_available: bool,
) -> SidecarResult<DispatchPlan> {
    let packs = list_model_packs(models_dir);
    let mlx_request = pack_id.ends_with("_mlx");
    let is_apple_silicon = matches!(platform, Platform::MacOsAppleSilicon);
    let mlx_can_run = mlx_request && is_apple_silicon && mlx_available;

    let resolved_id = if mlx_request && !mlx_can_run {
        // Fall back to the GGUF equivalent. We can't just strip the
        // `_mlx` suffix because upstream MLX builds sometimes use a
        // different base id than the GGUF build (e.g. SmolVLM2-256M
        // GGUF vs SmolVLM-256M-Instruct MLX). The fallback map in
        // the registry is the source of truth.
        let alt = gguf_fallback_for_mlx_pack(pack_id)
            .ok_or_else(|| SidecarError::ModelMissing(PathBuf::from(pack_id)))?;
        if !packs.iter().any(|p| p.id == alt) {
            return Err(SidecarError::ModelMissing(PathBuf::from(pack_id)));
        }
        alt.to_string()
    } else {
        pack_id.to_string()
    };

    let pack = packs
        .iter()
        .find(|p| p.id == resolved_id)
        .ok_or_else(|| SidecarError::ModelMissing(PathBuf::from(&resolved_id)))?;
    if pack.file_path.is_empty() {
        return Err(SidecarError::ModelMissing(PathBuf::from(&resolved_id)));
    }
    let model_path = models_dir.join(&pack.file_path);

    let (runtime, reason) = if mlx_can_run {
        (SidecarRuntime::MlxLm, DispatchReason::MlxNative)
    } else if mlx_request {
        (
            SidecarRuntime::LlamaServer,
            DispatchReason::MlxUnavailableFallback,
        )
    } else {
        (SidecarRuntime::LlamaServer, DispatchReason::LlamaServer)
    };

    let mmproj_path = if runtime == SidecarRuntime::LlamaServer {
        mmproj_for(&resolved_id).and_then(|m_id| {
            packs
                .iter()
                .find(|p| p.id == m_id)
                .map(|p| models_dir.join(&p.file_path))
        })
    } else {
        None
    };

    Ok(DispatchPlan {
        runtime,
        model_path,
        mmproj_path,
        resolved_pack_id: resolved_id,
        reason,
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
    let plan = plan_dispatch(pack_id, models_dir, platform, probe_mlx_available())?;
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
        SidecarRuntime::MlxLm => {
            let cfg = MlxSidecarConfig {
                python: PathBuf::from("python3"),
                model_path: plan.model_path.clone(),
                context_size: 4096,
                health_timeout: Duration::from_mins(1),
                extra_args: vec![],
            };
            let mut s = MlxSidecar::new(cfg);
            s.start()?;
            Ok(SidecarHandle::Mlx(s))
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
        let plan = plan_dispatch(
            "llm_sidecar_3b",
            dir.path(),
            Platform::LinuxX64,
            /* mlx_available = */ false,
        )
        .unwrap();
        assert_eq!(plan.runtime, SidecarRuntime::LlamaServer);
        assert_eq!(plan.reason, DispatchReason::LlamaServer);
        assert!(plan.mmproj_path.is_none());
    }

    /// On Apple Silicon with MLX installed, an `_mlx` pack must
    /// pick the MLX runtime.
    #[test]
    fn mlx_pack_on_apple_silicon_with_mlx_picks_mlx() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_dispatch(
            "vision_qwen35_4b_mlx",
            dir.path(),
            Platform::MacOsAppleSilicon,
            /* mlx_available = */ true,
        )
        .unwrap();
        assert_eq!(plan.runtime, SidecarRuntime::MlxLm);
        assert_eq!(plan.reason, DispatchReason::MlxNative);
        assert_eq!(plan.resolved_pack_id, "vision_qwen35_4b_mlx");
        // MLX runtime does NOT need a separate mmproj — MLX packs
        // ship the projector inside the model directory.
        assert!(plan.mmproj_path.is_none());
    }

    /// On Linux, an `_mlx` pack must fall back to the GGUF
    /// equivalent so the user still gets a working dispatcher.
    #[test]
    fn mlx_pack_on_linux_falls_back_to_llama_server() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_dispatch(
            "vision_qwen35_4b_mlx",
            dir.path(),
            Platform::LinuxX64,
            /* mlx_available = */ false,
        )
        .unwrap();
        assert_eq!(plan.runtime, SidecarRuntime::LlamaServer);
        assert_eq!(plan.reason, DispatchReason::MlxUnavailableFallback);
        assert_eq!(plan.resolved_pack_id, "vision_qwen35_4b");
        // Llama-server vision DOES require a mmproj file.
        assert!(plan.mmproj_path.is_some());
        assert!(plan
            .mmproj_path
            .as_ref()
            .unwrap()
            .ends_with("qwen2.5-vl-4b-mmproj-f16.gguf"));
    }

    /// On Apple Silicon WITHOUT MLX installed, the dispatcher must
    /// also fall back rather than spawn MLX and fail at runtime.
    #[test]
    fn mlx_pack_on_apple_silicon_without_mlx_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_dispatch(
            "vision_smolvlm_256m_mlx",
            dir.path(),
            Platform::MacOsAppleSilicon,
            /* mlx_available = */ false,
        )
        .unwrap();
        assert_eq!(plan.runtime, SidecarRuntime::LlamaServer);
        assert_eq!(plan.reason, DispatchReason::MlxUnavailableFallback);
        assert_eq!(plan.resolved_pack_id, "vision_smolvlm2_256m");
    }

    /// Unknown pack ids surface as `ModelMissing` rather than
    /// spawning anything.
    #[test]
    fn unknown_pack_id_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            plan_dispatch("does_not_exist", dir.path(), Platform::LinuxX64, false).unwrap_err();
        assert!(matches!(err, SidecarError::ModelMissing(_)));
    }

    /// Vision pack on llama-server must include the mmproj path.
    #[test]
    fn vision_pack_on_llama_server_resolves_mmproj() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_dispatch(
            "vision_smolvlm2_256m",
            dir.path(),
            Platform::LinuxX64,
            false,
        )
        .unwrap();
        assert_eq!(plan.runtime, SidecarRuntime::LlamaServer);
        assert!(plan.mmproj_path.is_some());
        assert!(plan
            .mmproj_path
            .as_ref()
            .unwrap()
            .ends_with("smolvlm2-256m-mmproj-f16.gguf"));
    }
}
