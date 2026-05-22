//! AI model-pack registry.
//!
//! Describes which model packs exist, which are built-in (always
//! available, zero-dep algorithms shipped inside `kcreate_ai`), and
//! which are optional ONNX/GGUF downloads. The Phase 2 implementation
//! treats "installed" as "file present in `models_dir`" — the actual
//! download flow is Phase 3.
//!
//! The list is *not* dynamically discovered from the filesystem: every
//! pack is a known entry with a canonical id. The filesystem check
//! only determines whether the file backing an optional pack already
//! exists.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Stable identifier strings for built-in / optional packs. Anything
/// shipped inside `kcreate_ai` itself is `BuiltIn`; anything that
/// requires an external file is `Onnx` (ONNX weights loaded in-process)
/// or `Sidecar` (weights loaded by the long-running LLM sidecar, e.g.
/// GGUF). The variant names follow Rust convention; the serde wire
/// format snake-cases them so the TypeScript layer sees
/// `"built_in" | "onnx" | "sidecar"` — see
/// `apps/desktop/shared/scene.ts::ModelPack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    /// Implemented in pure Rust inside `kcreate_ai`; nothing to
    /// download.
    BuiltIn,
    /// ONNX model loaded in-process via the `ort` crate (gated behind
    /// the `onnx_bg_removal` feature on the build that bundles it).
    Onnx,
    /// Weights consumed by the LLM sidecar (currently GGUF / llama.cpp).
    Sidecar,
}

/// Coarse category for the model-manager UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPackCategory {
    /// Core packs always available: bg-removal threshold, palette,
    /// upscale, smart-select.
    Core,
    /// Image-pro packs: neural bg-removal, neural upscale.
    ImagePro,
    /// Design-pro packs: LLM-driven design suggestions.
    DesignPro,
    /// Generation packs: diffusion (opt-in).
    Generation,
}

/// A single model-pack entry surfaced to the UI.
///
/// Field naming on the wire is `camelCase` to match every other
/// Phase 2 N-API type (`PreflightOptions`, `BatchJobStatus`,
/// `McpStatus`, …). The TypeScript mirror in
/// `apps/desktop/shared/scene.ts::ModelPack` uses the same casing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPack {
    pub id: String,
    pub name: String,
    pub category: ModelPackCategory,
    pub kind: ModelKind,
    pub capabilities: Vec<String>,
    /// On-disk size in bytes — `0` for built-in packs.
    pub size_bytes: u64,
    /// File this pack expects in `models_dir` (relative). Empty for
    /// built-in packs.
    pub file_path: String,
    /// Whether the pack is currently usable. For built-in packs this
    /// is always `true`; for optional packs this reflects whether
    /// `models_dir/file_path` exists.
    pub installed: bool,
}

/// Return the canonical list of model packs, with `installed`
/// computed against `models_dir`. Passing a `models_dir` that does
/// not exist is fine — every optional pack just shows up as
/// `installed: false`.
#[must_use]
pub fn list_model_packs(models_dir: &Path) -> Vec<ModelPack> {
    static_packs()
        .into_iter()
        .map(|mut p| {
            if p.kind == ModelKind::BuiltIn {
                p.installed = true;
            } else if !p.file_path.is_empty() {
                p.installed = models_dir.join(&p.file_path).exists();
            }
            p
        })
        .collect()
}

/// Check whether a specific pack is currently installed.
#[must_use]
pub fn is_installed(pack_id: &str, models_dir: &Path) -> bool {
    list_model_packs(models_dir)
        .iter()
        .find(|p| p.id == pack_id)
        .is_some_and(|p| p.installed)
}

/// Resolve the on-disk path for an optional pack. Returns `None` for
/// built-in packs (no file).
#[must_use]
pub fn pack_path(pack_id: &str, models_dir: &Path) -> Option<PathBuf> {
    static_packs()
        .into_iter()
        .find(|p| p.id == pack_id)
        .and_then(|p| {
            if p.file_path.is_empty() {
                None
            } else {
                Some(models_dir.join(p.file_path))
            }
        })
}

/// The canonical pack list. Keep this in lockstep with
/// `apps/desktop/shared/scene.ts::ModelPack` and the ModelManager UI.
fn static_packs() -> Vec<ModelPack> {
    vec![
        ModelPack {
            id: "bg_remove_threshold".into(),
            name: "Background Removal — Threshold".into(),
            category: ModelPackCategory::Core,
            kind: ModelKind::BuiltIn,
            capabilities: vec!["bg_remove".into()],
            size_bytes: 0,
            file_path: String::new(),
            installed: true,
        },
        ModelPack {
            id: "upscale_lanczos".into(),
            name: "Image Upscale — Lanczos3".into(),
            category: ModelPackCategory::Core,
            kind: ModelKind::BuiltIn,
            capabilities: vec!["upscale".into()],
            size_bytes: 0,
            file_path: String::new(),
            installed: true,
        },
        ModelPack {
            id: "palette_kmeans".into(),
            name: "Palette Extraction — k-means".into(),
            category: ModelPackCategory::Core,
            kind: ModelKind::BuiltIn,
            capabilities: vec!["palette".into(), "design_tokens".into()],
            size_bytes: 0,
            file_path: String::new(),
            installed: true,
        },
        ModelPack {
            id: "smart_select_flood".into(),
            name: "Smart Select — Flood Fill".into(),
            category: ModelPackCategory::Core,
            kind: ModelKind::BuiltIn,
            capabilities: vec!["smart_select".into()],
            size_bytes: 0,
            file_path: String::new(),
            installed: true,
        },
        ModelPack {
            id: "bg_remove_u2net".into(),
            name: "Background Removal — u²-net".into(),
            category: ModelPackCategory::ImagePro,
            kind: ModelKind::Onnx,
            capabilities: vec!["bg_remove".into()],
            size_bytes: 176_000_000,
            file_path: "u2net.onnx".into(),
            installed: false,
        },
        ModelPack {
            id: "upscale_esrgan".into(),
            name: "Image Upscale — ESRGAN".into(),
            category: ModelPackCategory::ImagePro,
            kind: ModelKind::Onnx,
            capabilities: vec!["upscale".into()],
            size_bytes: 67_000_000,
            file_path: "esrgan.onnx".into(),
            installed: false,
        },
        ModelPack {
            id: "screenshot_to_layout".into(),
            name: "Screenshot to Layout (edge+CCA)".into(),
            category: ModelPackCategory::DesignPro,
            kind: ModelKind::BuiltIn,
            capabilities: vec!["screenshot_to_layout".into()],
            size_bytes: 0,
            file_path: String::new(),
            installed: true,
        },
        ModelPack {
            id: "llm_sidecar_3b".into(),
            name: "Design LLM — 3B Instruct (GGUF)".into(),
            category: ModelPackCategory::DesignPro,
            kind: ModelKind::Sidecar,
            capabilities: vec!["design_suggestions".into(), "layer_naming".into()],
            size_bytes: 2_000_000_000,
            file_path: "design_llm.gguf".into(),
            installed: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_returns_full_set() {
        let dir = tempfile::tempdir().unwrap();
        let packs = list_model_packs(dir.path());
        assert_eq!(packs.len(), 8);
        assert!(packs.iter().any(|p| p.id == "bg_remove_threshold"));
        assert!(packs.iter().any(|p| p.id == "upscale_lanczos"));
        assert!(packs.iter().any(|p| p.id == "palette_kmeans"));
        assert!(packs.iter().any(|p| p.id == "smart_select_flood"));
        assert!(packs.iter().any(|p| p.id == "bg_remove_u2net"));
        assert!(packs.iter().any(|p| p.id == "upscale_esrgan"));
        assert!(packs.iter().any(|p| p.id == "llm_sidecar_3b"));
    }

    #[test]
    fn builtin_packs_are_always_installed() {
        let dir = tempfile::tempdir().unwrap();
        let packs = list_model_packs(dir.path());
        for p in packs.iter().filter(|p| p.kind == ModelKind::BuiltIn) {
            assert!(p.installed, "{} should be installed", p.id);
            assert!(p.file_path.is_empty());
            assert_eq!(p.size_bytes, 0);
        }
    }

    #[test]
    fn onnx_pack_is_installed_only_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        // Without the file: installed == false.
        assert!(!is_installed("bg_remove_u2net", dir.path()));
        // With the file: installed == true.
        fs::write(dir.path().join("u2net.onnx"), b"weights").unwrap();
        assert!(is_installed("bg_remove_u2net", dir.path()));
    }

    #[test]
    fn unknown_pack_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_installed("does_not_exist", dir.path()));
    }

    #[test]
    fn pack_path_resolves_to_models_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = pack_path("bg_remove_u2net", dir.path()).expect("optional pack has a path");
        assert!(path.ends_with("u2net.onnx"));
        assert!(path.starts_with(dir.path()));
    }

    #[test]
    fn pack_path_is_none_for_builtin() {
        let dir = tempfile::tempdir().unwrap();
        assert!(pack_path("bg_remove_threshold", dir.path()).is_none());
    }

    #[test]
    fn pack_serialises_to_camelcase_wire_format() {
        let dir = tempfile::tempdir().unwrap();
        let packs = list_model_packs(dir.path());
        // Round-trip a single pack through JSON.
        let raw = serde_json::to_string(&packs[0]).unwrap();
        let p: ModelPack = serde_json::from_str(&raw).unwrap();
        assert_eq!(p, packs[0]);
        // Wire-format lockstep: the field names must match
        // `apps/desktop/shared/scene.ts::ModelPack` (camelCase). If you
        // rename a field, update the TS interface and keep this test
        // passing.
        assert!(
            raw.contains("\"sizeBytes\""),
            "expected camelCase wire key `sizeBytes`, got {raw}"
        );
        assert!(
            raw.contains("\"filePath\""),
            "expected camelCase wire key `filePath`, got {raw}"
        );
        assert!(
            !raw.contains("\"size_bytes\""),
            "snake_case `size_bytes` must not leak onto the wire: {raw}"
        );
        assert!(
            !raw.contains("\"file_path\""),
            "snake_case `file_path` must not leak onto the wire: {raw}"
        );
    }

    /// Wire-format lockstep: the strings on the wire must be exactly
    /// `built_in` / `onnx` / `sidecar` so the TypeScript layer's
    /// discriminated union (`apps/desktop/shared/scene.ts::ModelKind`)
    /// stays in sync with the Rust serde encoding. If you rename a
    /// variant, fix the TS type AND keep this test passing.
    #[test]
    fn model_kind_serde_matches_typescript_wire_format() {
        assert_eq!(
            serde_json::to_string(&ModelKind::BuiltIn).unwrap(),
            "\"built_in\""
        );
        assert_eq!(serde_json::to_string(&ModelKind::Onnx).unwrap(), "\"onnx\"");
        assert_eq!(
            serde_json::to_string(&ModelKind::Sidecar).unwrap(),
            "\"sidecar\""
        );
        // And round-trip back to the same variants.
        let parsed: ModelKind = serde_json::from_str("\"built_in\"").unwrap();
        assert_eq!(parsed, ModelKind::BuiltIn);
        let parsed: ModelKind = serde_json::from_str("\"sidecar\"").unwrap();
        assert_eq!(parsed, ModelKind::Sidecar);
    }
}
