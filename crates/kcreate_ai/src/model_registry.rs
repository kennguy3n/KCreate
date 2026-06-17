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

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

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
    /// Vision packs: multimodal (VLM) models that consume images and
    /// emit text — alt-text, design critique, screenshot
    /// classification, brand extraction, smart-crop, style
    /// description. Loaded through the same sidecar lifecycle as
    /// text-only LLMs but always paired with an mmproj file.
    Vision,
    /// Generation packs: diffusion (opt-in, Tier 2+ with GPU only).
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
    /// Canonical download URL the user can fetch the weights from
    /// **out-of-band**. KCreate never reaches out to this URL itself
    /// — the editor is network-free (see `local_first.rs`) — but the
    /// UI shows it so the user knows where to grab the weights from
    /// before pointing the installer at the downloaded file. Empty
    /// for built-in packs.
    pub download_url: String,
    /// Hex-encoded SHA-256 of the canonical weights file. The
    /// installer rejects any file whose SHA-256 doesn't match this,
    /// so swapping in a corrupted or tampered file is structurally
    /// impossible. Empty for built-in packs.
    pub sha256: String,
}

/// Canonical SHA-256 hashes for optional model packs that ship in
/// **stable releases**. Keep this table empty on `main`; the release
/// pipeline (Phase 3, see PR #8 release-engineering follow-up) is
/// the only place that fills it in, by editing this constant as part
/// of the release commit and running [`assert_canonical_hashes_well_formed`]
/// to confirm every entry is a syntactically valid 64-character
/// lowercase hex digest.
///
/// Why a separate table from [`static_packs`]: this layout keeps the
/// structural pack catalogue (ids, URLs, sizes, capabilities) free of
/// release-engineering churn. A release that pins
/// `bg_remove_u2net` only edits the canonical-hashes table; the
/// catalogue stays byte-identical. CI rejects malformed entries via
/// the test at the bottom of this module.
///
/// IMPORTANT: do *not* invent hashes. The installer rejects mismatches
/// (`InstallError::ChecksumMismatch`), so a guessed hash would brick
/// every install of that pack. Always paste hashes computed from the
/// canonical upstream artefact (see `ModelPack::download_url`).
const CANONICAL_PACK_HASHES: &[(&str, &str)] = &[
    // Stable Diffusion 1.5 (fused, fp16) from the Comfy-Org archive
    // mirror referenced by `image_gen_sd15`'s `download_url`. This is
    // the smallest real generation model the registry ships, so it is
    // the one we pin first: download + checksum + run is exercised
    // end-to-end in CI-adjacent integration tests and by hand.
    (
        "image_gen_sd15",
        "e9476a13728cd75d8279f6ec8bad753a66a1957ca375a1464dc63b37db6e3916",
    ),
    // Bonsai Image Ternary 4B primary transformer weights. Pinned to
    // the upstream `prism-ml` LFS oids of each variant's primary file
    // (the `download_url` below): the mflux-packed safetensors for the
    // Apple-Silicon MLX build and the gemlite int2 `state_dict.pt` for
    // the CUDA build. Companion encoder / VAE files ride on the
    // runner's own extra args (see `phase4::parse_bonsai_extra_args`).
    (
        "image_gen_bonsai_mlx_4b",
        "b21737bdf02690b7d662907781c4dc8b8bf22a2c98b823b1ca3336f48371a84f",
    ),
    (
        "image_gen_bonsai_gemlite_4b",
        "a3a7df8a90374fea24afce3b36f00b4c728d0254717143d61f912a7b3070e7ac",
    ),
    // Pinned at release time. Example shape (commented out):
    // ("bg_remove_u2net", "0123456789abcdef…"),
];

/// Look up the canonical hash for a pack id from
/// [`CANONICAL_PACK_HASHES`]. Returns `""` when the pack hasn't been
/// pinned yet — matches the pre-PR registry semantics so the
/// installer's "do not enforce" branch still triggers for unpinned
/// packs.
#[must_use]
fn canonical_hash_for(pack_id: &str) -> &'static str {
    CANONICAL_PACK_HASHES
        .iter()
        .find(|(id, _)| *id == pack_id)
        .map_or("", |(_, h)| *h)
}

/// Return the canonical list of model packs, with `installed`
/// computed against `models_dir` and `sha256` overlaid from
/// [`CANONICAL_PACK_HASHES`]. Passing a `models_dir` that does not
/// exist is fine — every optional pack just shows up as
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
            let canonical = canonical_hash_for(&p.id);
            if !canonical.is_empty() {
                p.sha256 = canonical.into();
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
///
/// SHA-256 hashes for the optional packs are pinned to the *exact*
/// upstream release artefacts the README documents. The installer
/// (see [`install_model_pack`]) rejects any file whose hash doesn't
/// match, so users can't accidentally install corrupted or
/// substituted weights — and the editor itself never reaches over
/// the network to fetch them (local-first invariant: see
/// `kcreate_tests/local_first.rs`).
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
            download_url: String::new(),
            sha256: String::new(),
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
            download_url: String::new(),
            sha256: String::new(),
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
            download_url: String::new(),
            sha256: String::new(),
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
            download_url: String::new(),
            sha256: String::new(),
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
            // Canonical u²-net ONNX export hosted by the original
            // authors at danielgatis/rembg.
            //
            // `sha256` is intentionally empty in this Phase 2 build.
            // The installer (`install_model_pack`) treats an empty
            // expected hash as "do not enforce" and writes the file
            // through unchecked but records the actual hash so the
            // UI can surface it. Canonical hashes will be backfilled
            // by the publishing pipeline once the model-pack
            // distribution channel is finalised — *not* by guessing
            // them at code-review time. See the doc comment on
            // [`ModelPack::sha256`] for the verification contract.
            download_url:
                "https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2net.onnx".into(),
            sha256: String::new(),
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
            // Real-ESRGAN x4 plus ONNX export. See `bg_remove_u2net`
            // above for the empty-`sha256` policy.
            download_url:
                "https://huggingface.co/PINTO0309/Real-ESRGAN/resolve/main/RealESRGAN_x4plus_anime_6B.onnx".into(),
            sha256: String::new(),
        },
        ModelPack {
            // Phase 3 Tasks 9-10 — SAM (Segment Anything) point-prompt
            // segmentation backend. The bridge consumes this pack
            // via [`segment::SegmentBackend::Sam`] when the
            // `onnx_segment` feature is enabled. Sized for the fused
            // MobileSAM single-file ONNX export (~40 MB) — the full
            // ViT-B variant would be a separate pack with the same
            // `segment` capability.
            id: "segment_sam".into(),
            name: "Segment Anything — MobileSAM".into(),
            category: ModelPackCategory::ImagePro,
            kind: ModelKind::Onnx,
            capabilities: vec!["segment".into()],
            size_bytes: 40_000_000,
            file_path: "sam.onnx".into(),
            installed: false,
            // MobileSAM fused-decoder ONNX export. See
            // `bg_remove_u2net` above for the empty-`sha256` policy.
            download_url:
                "https://huggingface.co/ChaoningZhang/MobileSAM/resolve/main/mobile_sam.onnx".into(),
            sha256: String::new(),
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
            download_url: String::new(),
            sha256: String::new(),
        },
        ModelPack {
            // Phase 4 follow-up Block D — text-region detector
            // backing the "Insert as text layer" affordance in
            // AIAssistPanel. BuiltIn because the detector is pure
            // CV (threshold + connected components + line
            // grouping) and ships with the editor — no weights,
            // no download. A future high-accuracy OCR (ONNX or
            // Tesseract WASM) would land here as a separate pack
            // with the same `ocr` capability so the dispatcher
            // can prefer it when installed.
            id: "ocr_heuristic".into(),
            name: "Text Region Detection — Heuristic".into(),
            category: ModelPackCategory::Core,
            kind: ModelKind::BuiltIn,
            capabilities: vec!["ocr".into()],
            size_bytes: 0,
            file_path: String::new(),
            installed: true,
            download_url: String::new(),
            sha256: String::new(),
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
            // Llama 3.2 3B Instruct Q4_K_M GGUF (bartowski mirror,
            // generated from Meta's official weights with llama.cpp
            // convert_hf_to_gguf.py). See `bg_remove_u2net` above
            // for the empty-`sha256` policy.
            download_url:
                "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf".into(),
            sha256: String::new(),
        },
        // ---- Phase 12 Block A: Ternary-Bonsai text packs ----
        //
        // Tier-aware default LLMs that replace `llm_sidecar_3b` as
        // the recommended text model (see `recommended_llm_pack`).
        // All three are standard GGUF (Q2_0 ternary quantisation,
        // ~1.58 bits/weight) and load directly in llama-server with
        // Metal / CUDA / Vulkan acceleration on every supported
        // platform — no Python, no MLX. Sizes are verified directly
        // from the upstream Hugging Face repos.
        ModelPack {
            id: "llm_bonsai_1_7b".into(),
            name: "Design LLM — Ternary-Bonsai 1.7B (GGUF Q2_0)".into(),
            category: ModelPackCategory::DesignPro,
            kind: ModelKind::Sidecar,
            capabilities: vec!["design_suggestions".into(), "layer_naming".into()],
            // 463 MB on disk per `huggingface-cli ls` against
            // `prism-ml/Ternary-Bonsai-1.7B-gguf` (Q2_0 file). Fits
            // comfortably in Tier 0 (4 GB) RAM budgets.
            size_bytes: 463_000_000,
            file_path: "Ternary-Bonsai-1.7B-Q2_0.gguf".into(),
            installed: false,
            download_url:
                "https://huggingface.co/prism-ml/Ternary-Bonsai-1.7B-gguf/resolve/main/Ternary-Bonsai-1.7B-Q2_0.gguf".into(),
            sha256: String::new(),
        },
        ModelPack {
            id: "llm_bonsai_4b".into(),
            name: "Design LLM — Ternary-Bonsai 4B (GGUF Q2_0)".into(),
            category: ModelPackCategory::DesignPro,
            kind: ModelKind::Sidecar,
            capabilities: vec!["design_suggestions".into(), "layer_naming".into()],
            // 1.07 GB on disk per `huggingface-cli ls` against
            // `prism-ml/Ternary-Bonsai-4B-gguf` (Q2_0 file). Tier 1
            // (8 GB) sweet spot.
            size_bytes: 1_070_000_000,
            file_path: "Ternary-Bonsai-4B-Q2_0.gguf".into(),
            installed: false,
            download_url:
                "https://huggingface.co/prism-ml/Ternary-Bonsai-4B-gguf/resolve/main/Ternary-Bonsai-4B-Q2_0.gguf".into(),
            sha256: String::new(),
        },
        ModelPack {
            id: "llm_bonsai_8b".into(),
            name: "Design LLM — Ternary-Bonsai 8B (GGUF Q2_0)".into(),
            category: ModelPackCategory::DesignPro,
            kind: ModelKind::Sidecar,
            capabilities: vec!["design_suggestions".into(), "layer_naming".into()],
            // 2.18 GB on disk per `huggingface-cli ls` against
            // `prism-ml/Ternary-Bonsai-8B-gguf` (Q2_0 file). Tier 2
            // (16+ GB) target.
            size_bytes: 2_180_000_000,
            file_path: "Ternary-Bonsai-8B-Q2_0.gguf".into(),
            installed: false,
            download_url:
                "https://huggingface.co/prism-ml/Ternary-Bonsai-8B-gguf/resolve/main/Ternary-Bonsai-8B-Q2_0.gguf".into(),
            sha256: String::new(),
        },
        // ---- Phase 4 vision packs (GGUF + mmproj) ----
        //
        // Each vision pack consists of TWO file entries — the model
        // weights (`*-q4_k_m.gguf`) and the companion multimodal
        // projector (`*-mmproj.gguf`). The sidecar driver loads
        // both via `--model` and `--mmproj` (see
        // `SidecarConfig::mmproj_path`). The UI shows the model
        // entry; the mmproj entry is installed in lockstep but
        // hidden from the primary listing via the `mmproj`
        // capability marker.
        ModelPack {
            id: "vision_smolvlm2_256m".into(),
            name: "Vision (CPU) — SmolVLM2-256M Instruct".into(),
            category: ModelPackCategory::Vision,
            kind: ModelKind::Sidecar,
            capabilities: vec!["vision".into(), "alt_text".into()],
            // ~180 MB Q4_K_S weights. Runs on CPU with reasonable
            // latency on every supported tier (including Tier 0
            // laptops), which is why this is the default vision
            // recommendation for Tier 0/1 in
            // `recommended_vision_pack`.
            size_bytes: 180_000_000,
            file_path: "smolvlm2-256m-q4_k_s.gguf".into(),
            installed: false,
            download_url:
                "https://huggingface.co/ggml-org/SmolVLM-256M-Instruct-GGUF/resolve/main/SmolVLM-256M-Instruct-Q4_K_S.gguf".into(),
            sha256: String::new(),
        },
        ModelPack {
            id: "vision_smolvlm2_256m_mmproj".into(),
            name: "Vision (CPU) — SmolVLM2-256M mmproj".into(),
            category: ModelPackCategory::Vision,
            kind: ModelKind::Sidecar,
            // `mmproj` is the capability marker the dispatcher uses
            // to skip projector entries when enumerating models for
            // the chat selector — projector files are never loaded
            // on their own, only alongside the paired weights.
            capabilities: vec!["mmproj".into()],
            size_bytes: 90_000_000,
            file_path: "smolvlm2-256m-mmproj-f16.gguf".into(),
            installed: false,
            download_url:
                "https://huggingface.co/ggml-org/SmolVLM-256M-Instruct-GGUF/resolve/main/mmproj-SmolVLM-256M-Instruct-F16.gguf".into(),
            sha256: String::new(),
        },
        ModelPack {
            id: "vision_qwen25vl_7b".into(),
            name: "Vision (GPU) — Qwen2.5-VL 7B Instruct".into(),
            category: ModelPackCategory::Vision,
            kind: ModelKind::Sidecar,
            capabilities: vec![
                "vision".into(),
                "design_critique".into(),
                "alt_text".into(),
                "brand_extract".into(),
                "smart_crop".into(),
                "style_describe".into(),
            ],
            size_bytes: 4_700_000_000,
            file_path: "qwen2.5-vl-7b-instruct-q4_k_m.gguf".into(),
            installed: false,
            download_url:
                "https://huggingface.co/ggml-org/Qwen2.5-VL-7B-Instruct-GGUF/resolve/main/Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf".into(),
            sha256: String::new(),
        },
        ModelPack {
            id: "vision_qwen25vl_7b_mmproj".into(),
            name: "Vision (GPU) — Qwen2.5-VL 7B mmproj (F16)".into(),
            category: ModelPackCategory::Vision,
            kind: ModelKind::Sidecar,
            capabilities: vec!["mmproj".into()],
            size_bytes: 1_350_000_000,
            file_path: "mmproj-qwen2.5-vl-7b-instruct-f16.gguf".into(),
            installed: false,
            download_url:
                "https://huggingface.co/ggml-org/Qwen2.5-VL-7B-Instruct-GGUF/resolve/main/mmproj-Qwen2.5-VL-7B-Instruct-F16.gguf".into(),
            sha256: String::new(),
        },
        // Phase 12 Block A removed the MLX vision packs
        // (`vision_smolvlm_256m_mlx`, `vision_qwen25vl_7b_mlx`) when
        // we consolidated text + vision on llama-server. The GGUF
        // entries above run with Metal acceleration on Apple
        // Silicon and the registry no longer surfaces MLX-only
        // alternatives.

        // ---- Phase 4 image generation packs (FLUX) ----
        //
        // Generation models are loaded by an entirely separate
        // sidecar (`crate::image_gen::ImageGenSidecar`) running
        // `sd-server` from stable-diffusion.cpp — *not* llama-server.
        // They are gated to Tier 2+ with GPU; the UI hides them on
        // lower tiers (see `DeviceTier::image_generation_allowed`).
        // Phase 12 Block B replaced the original Python diffusers
        // server with `sd-server` so there is no Python in the image
        // generation path either.
        ModelPack {
            id: "image_gen_flux_klein_4b".into(),
            name: "Image Generation — FLUX Klein 4B (GGUF)".into(),
            category: ModelPackCategory::Generation,
            kind: ModelKind::Sidecar,
            capabilities: vec!["image_generation".into()],
            size_bytes: 2_500_000_000,
            file_path: "flux-2-klein-4b-Q4_0.gguf".into(),
            installed: false,
            download_url:
                "https://huggingface.co/themindstudio/FLUX-Klein-4B-GGUF/resolve/main/flux-klein-4b-Q4_0.gguf".into(),
            sha256: String::new(),
        },
        // Smallest real generation model the registry ships:
        // Stable Diffusion 1.5, fused fp16 `.safetensors` (~2.0 GB).
        // Unlike FLUX (a *standalone* diffusion model that needs
        // separate CLIP / T5 / VAE files), an SD 1.x checkpoint
        // bundles its CLIP + VAE, so sd-server loads it through `-m`
        // with no companion encoder paths. The bridge selects the
        // `-m` flag for this pack via `generation_pack_is_fused`.
        // This is the pack used for the download + checksum + run
        // proof because it is small enough to fetch and verify in a
        // CI-sized environment while still producing a recognisable
        // hero image.
        ModelPack {
            id: "image_gen_sd15".into(),
            name: "Image Generation — Stable Diffusion 1.5 (fp16)".into(),
            category: ModelPackCategory::Generation,
            kind: ModelKind::Sidecar,
            capabilities: vec!["image_generation".into()],
            size_bytes: 2_132_696_762,
            file_path: "stable-diffusion-v1-5-pruned-emaonly-fp16.safetensors".into(),
            installed: false,
            download_url:
                "https://huggingface.co/Comfy-Org/stable-diffusion-v1-5-archive/resolve/main/v1-5-pruned-emaonly-fp16.safetensors".into(),
            sha256: String::new(),
        },
        // Bonsai Image Ternary 4B — a ternary-quantized FLUX.2 Klein
        // published by `prism-ml` in two accelerator-specific builds.
        // Neither loads in sd-server/stable-diffusion.cpp: they ship
        // their own runtimes (mflux on Apple Silicon, gemlite/HQQ
        // kernels on CUDA), so `generation_engine_for` routes them to
        // an external Bonsai runner instead of the sd-server sidecar
        // (see `crates/kcreate_bridge/src/phase4.rs`). They are opt-in
        // via the generation-model selector — SD 1.5 stays the default
        // fallback whenever the matching accelerator or runner is
        // absent. Each pack's `file_path` is the primary transformer
        // weight; the companion text-encoder / VAE files are passed to
        // the runner through `KCREATE_BONSAI_*_EXTRA_ARGS`.
        //
        // Apple-Silicon (MLX 2-bit): the mflux-packed transformer.
        ModelPack {
            id: "image_gen_bonsai_mlx_4b".into(),
            name: "Image Generation — Bonsai Image Ternary 4B (MLX 2-bit · Apple Silicon)".into(),
            category: ModelPackCategory::Generation,
            kind: ModelKind::Sidecar,
            capabilities: vec!["image_generation".into()],
            size_bytes: 1_425_271_472,
            file_path: "bonsai-image-ternary-4b-mlx-2bit-transformer.safetensors".into(),
            installed: false,
            download_url:
                "https://huggingface.co/prism-ml/bonsai-image-ternary-4B-mlx-2bit/resolve/main/transformer-packed-mflux/diffusion_pytorch_model.safetensors".into(),
            sha256: String::new(),
        },
        // CUDA Windows/Linux (GemLite int2): the gemlite state_dict.
        ModelPack {
            id: "image_gen_bonsai_gemlite_4b".into(),
            name: "Image Generation — Bonsai Image Ternary 4B (GemLite 2-bit · CUDA GPU)".into(),
            category: ModelPackCategory::Generation,
            kind: ModelKind::Sidecar,
            capabilities: vec!["image_generation".into()],
            size_bytes: 1_540_457_482,
            file_path: "bonsai-image-ternary-4b-gemlite-2bit-transformer.pt".into(),
            installed: false,
            download_url:
                "https://huggingface.co/prism-ml/bonsai-image-ternary-4B-gemlite-2bit/resolve/main/transformer-gemlite-int2/state_dict.pt".into(),
            sha256: String::new(),
        },
    ]
}

/// Return the canonical mmproj pack id that pairs with `pack_id`, or
/// `None` if `pack_id` is not a vision model (or is itself the
/// mmproj entry). The sidecar dispatcher uses this to resolve both
/// files when starting a vision sidecar.
#[must_use]
pub fn mmproj_for(pack_id: &str) -> Option<&'static str> {
    match pack_id {
        "vision_smolvlm2_256m" => Some("vision_smolvlm2_256m_mmproj"),
        "vision_qwen25vl_7b" => Some("vision_qwen25vl_7b_mmproj"),
        _ => None,
    }
}

/// Migrate a legacy pack id from a saved project preference (or a
/// stale settings JSON) to the current registry equivalent. Returns
/// `None` when `pack_id` is already current — callers treat that as
/// "no rewrite needed" rather than a missing pack.
///
/// Phase 12 Block A dropped the MLX runtime, which removed three
/// pack ids that previously shipped:
///
/// * `vision_smolvlm_256m_mlx`   → `vision_smolvlm2_256m`
/// * `vision_qwen25vl_7b_mlx`    → `vision_qwen25vl_7b`
/// * `image_gen_flux_klein_mlx`  → `image_gen_flux_klein_4b`
///
/// Phase 4–11 installations wrote those ids into project files and
/// the per-user settings store; without this table they would
/// surface as `SidecarError::ModelMissing` errors on first launch
/// after the upgrade. The bridge entry points
/// (`vision_start`, `image_gen_start`, `vision_mmproj_for`) call
/// this helper before any lookup so the migration is transparent
/// to the renderer — they also log a one-time deprecation notice
/// so the model-manager UI can prompt the user to re-pick.
///
/// New pack additions in future phases that supersede an existing
/// id should extend this table rather than introducing a parallel
/// migration mechanism.
#[must_use]
pub fn migrate_legacy_pack_id(pack_id: &str) -> Option<&'static str> {
    match pack_id {
        "vision_smolvlm_256m_mlx" => Some("vision_smolvlm2_256m"),
        "vision_qwen25vl_7b_mlx" => Some("vision_qwen25vl_7b"),
        "image_gen_flux_klein_mlx" => Some("image_gen_flux_klein_4b"),
        _ => None,
    }
}

/// Recommend a vision pack for the given (tier, platform). Returns
/// the canonical pack id the model-manager UI should highlight as
/// "best for this machine".
///
/// Phase 12 Block A removed the platform branch: every tier uses
/// the GGUF vision packs through llama-server, which has full Metal
/// acceleration on Apple Silicon. The `_platform` argument stays in
/// the signature for forward compatibility (a future Rust-native
/// runtime may want to pick differently per OS) but is currently
/// unused.
///
/// - Tier 0 / 1: SmolVLM2-256M — runs comfortably on CPU.
/// - Tier 2 / 3: Qwen2.5-VL — the larger model is worth the cost.
///
/// Returns `None` only when the tier does not allow vision at all,
/// which is currently never (see [`DeviceTier::vision_model_allowed`]).
#[must_use]
pub fn recommended_vision_pack(
    tier: kcreate_core::config::DeviceTier,
    _platform: kcreate_core::config::Platform,
) -> Option<&'static str> {
    use kcreate_core::config::DeviceTier::{Tier0, Tier1, Tier2, Tier3};
    if !tier.vision_model_allowed() {
        return None;
    }
    Some(match tier {
        Tier0 | Tier1 => "vision_smolvlm2_256m",
        Tier2 | Tier3 => "vision_qwen25vl_7b",
    })
}

/// Recommend a text LLM pack.
///
/// Phase 12 Block A made this tier-aware against the Ternary-Bonsai
/// GGUF family (Q2_0 / 1.58-bit ternary quantisation). Each tier
/// gets the largest Bonsai model that fits its RAM envelope:
///
/// - Tier 0 → 1.7B (~460 MB on disk, runs on 4 GB RAM)
/// - Tier 1 → 4B   (~1.1 GB on disk, runs on 8 GB RAM)
/// - Tier 2 / 3 → 8B (~2.2 GB on disk, runs on 16+ GB RAM)
///
/// `llm_sidecar_3b` (Llama 3.2 3B Q4_K_M) stays in the registry as
/// an alternative for users who already have the GGUF cached or who
/// prefer a more conventional architecture. The `_platform` argument
/// is retained for API compatibility — every recommendation is the
/// same GGUF across Linux / Windows / macOS once the MLX-only
/// alternatives were removed.
#[must_use]
pub fn recommended_llm_pack(
    tier: kcreate_core::config::DeviceTier,
    _platform: kcreate_core::config::Platform,
) -> Option<&'static str> {
    use kcreate_core::config::DeviceTier::{Tier0, Tier1, Tier2, Tier3};
    Some(match tier {
        Tier0 => "llm_bonsai_1_7b",
        Tier1 => "llm_bonsai_4b",
        Tier2 | Tier3 => "llm_bonsai_8b",
    })
}

/// Recommend an image-generation pack, or `None` when the device
/// is below the Tier 2 + GPU gate. The model-manager UI calls this
/// AFTER checking [`kcreate_core::config::RuntimeConfig::image_generation_allowed`];
/// the function also returns `None` for sub-Tier-2 devices as a
/// belt-and-braces check.
///
/// Phase 12 Block A removed the Apple-Silicon-only MLX branch.
/// `sd-server` (stable-diffusion.cpp) loads both packs with Metal
/// acceleration on Apple Silicon and CUDA / Vulkan elsewhere, so a
/// single platform-agnostic recommendation works everywhere.
///
/// The recommendation is tier-aware: Tier 2 machines (the floor for
/// image generation) get the small, fully-verified SD 1.5 checkpoint
/// (`image_gen_sd15`, ~2.0 GB, fits in RAM and fetches + checksums
/// quickly); Tier 3 machines, which have the headroom for a larger
/// standalone diffusion model, get FLUX.2 Klein 4B.
#[must_use]
pub fn recommended_generation_pack(
    tier: kcreate_core::config::DeviceTier,
    _platform: kcreate_core::config::Platform,
) -> Option<&'static str> {
    use kcreate_core::config::DeviceTier;
    if !tier.image_generation_allowed() {
        return None;
    }
    Some(match tier {
        DeviceTier::Tier3 => "image_gen_flux_klein_4b",
        _ => "image_gen_sd15",
    })
}

/// Whether a generation pack's weights file is a *fused* full
/// checkpoint (SD 1.x / SD2 / SDXL `.safetensors`, which bundles
/// CLIP + VAE and loads via sd-server's `-m`) rather than a
/// *standalone* diffusion model (FLUX / SD3-style, which loads via
/// `--diffusion-model` with text encoders + VAE supplied through
/// `KCREATE_SD_SERVER_EXTRA_ARGS`).
///
/// The bridge uses this to pick the correct sd-server CLI flag when
/// it builds the [`crate::DiffusionSidecarConfig`]. Keyed by pack id
/// rather than a wire field so the renderer-facing `ModelPack` shape
/// is unchanged — mirrors the existing `mmproj_for` / `resolve_pack_id`
/// per-pack lookups.
#[must_use]
pub fn generation_pack_is_fused_checkpoint(pack_id: &str) -> bool {
    matches!(pack_id, "image_gen_sd15")
}

/// The local inference engine that runs a given image-generation
/// pack. `sd-server` (stable-diffusion.cpp) drives SD 1.5 and FLUX.2
/// Klein; the two Bonsai Image Ternary 4B variants ship their own
/// accelerator-specific runtimes that sd-server cannot load, so they
/// route to an external Bonsai runner instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationEngine {
    /// stable-diffusion.cpp `sd-server` — runs on every platform.
    SdCpp,
    /// Bonsai Image Ternary 4B, MLX 2-bit build (Apple Silicon /
    /// mflux runtime).
    BonsaiMlx,
    /// Bonsai Image Ternary 4B, GemLite 2-bit build (CUDA GPU /
    /// gemlite + HQQ runtime).
    BonsaiGemlite,
}

impl GenerationEngine {
    /// Stable wire string surfaced through the image-gen status IPC so
    /// the UI can report the active engine honestly. Kept in lockstep
    /// with `apps/desktop/shared/scene.ts`'s `ImageGenEngine` union.
    #[must_use]
    pub fn as_wire_str(self) -> &'static str {
        match self {
            Self::SdCpp => "sd_cpp",
            Self::BonsaiMlx => "bonsai_mlx",
            Self::BonsaiGemlite => "bonsai_gemlite",
        }
    }

    /// Whether this engine can run on `platform`. `sd-server` runs
    /// everywhere; the Bonsai builds are accelerator-specific (MLX
    /// needs Apple Silicon, GemLite needs a CUDA GPU on Windows or
    /// x86-64 Linux). The bridge uses this to decide whether a Bonsai
    /// request can be honoured or must fall back to SD 1.5.
    #[must_use]
    pub fn supports_platform(self, platform: kcreate_core::config::Platform) -> bool {
        use kcreate_core::config::Platform;
        match self {
            Self::SdCpp => true,
            Self::BonsaiMlx => matches!(platform, Platform::MacOsAppleSilicon),
            Self::BonsaiGemlite => matches!(platform, Platform::WindowsX64 | Platform::LinuxX64),
        }
    }
}

/// The inference engine that runs `pack_id`. Keyed by pack id (like
/// [`generation_pack_is_fused_checkpoint`] and `mmproj_for`) so the
/// renderer-facing `ModelPack` shape stays unchanged. Any unknown or
/// non-Bonsai pack falls through to [`GenerationEngine::SdCpp`].
#[must_use]
pub fn generation_engine_for(pack_id: &str) -> GenerationEngine {
    match pack_id {
        "image_gen_bonsai_mlx_4b" => GenerationEngine::BonsaiMlx,
        "image_gen_bonsai_gemlite_4b" => GenerationEngine::BonsaiGemlite,
        _ => GenerationEngine::SdCpp,
    }
}

/// The Bonsai Image Ternary 4B variant pack id that matches
/// `platform`, or `None` on platforms without a supported Bonsai
/// accelerator (Intel macOS, ARM Linux).
///
/// This is deliberately **NOT** wired into
/// [`recommended_generation_pack`]: SD 1.5 (and FLUX on Tier 3)
/// remains the default recommendation, and Bonsai is opt-in via the
/// generation-model selector. Keeping the MLX variant out of every
/// `recommended_*` return also preserves the Phase 12 invariant that
/// no recommendation ends in `_mlx`.
#[must_use]
pub fn bonsai_image_variant_for_platform(
    platform: kcreate_core::config::Platform,
) -> Option<&'static str> {
    use kcreate_core::config::Platform;
    match platform {
        Platform::MacOsAppleSilicon => Some("image_gen_bonsai_mlx_4b"),
        Platform::WindowsX64 | Platform::LinuxX64 => Some("image_gen_bonsai_gemlite_4b"),
        Platform::MacOsIntel | Platform::LinuxArm64 => None,
    }
}

/// Errors from [`install_model_pack`] / [`uninstall_model_pack`].
#[derive(Debug, Error)]
pub enum InstallError {
    #[error("unknown model pack id: {0}")]
    UnknownPack(String),
    /// Built-in packs (algorithm shipped inside `kcreate_ai` itself)
    /// have no file on disk to install / uninstall.
    #[error("model pack {0} is built-in and has no installable file")]
    BuiltIn(String),
    /// Source file did not exist or was unreadable.
    #[error("io error reading source file {path}: {source}")]
    SourceIo {
        path: String,
        #[source]
        source: io::Error,
    },
    /// Failed to write the destination (or its parent directory).
    #[error("io error writing destination {path}: {source}")]
    DestIo {
        path: String,
        #[source]
        source: io::Error,
    },
    /// The pack carries a canonical SHA-256 hash and the source file
    /// did not match. The installer never writes the destination if
    /// this check fails — there is no partial-write window.
    #[error("checksum mismatch for pack {pack_id}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        pack_id: String,
        expected: String,
        actual: String,
    },
}

/// Outcome of a successful [`install_model_pack`] call. The bridge
/// hands this back to the UI so it can show "Installed (verified)"
/// when the canonical hash matched, or "Installed (unverified —
/// hash recorded as XXX)" when the registry didn't carry a pinned
/// hash yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub pack_id: String,
    /// Hex-encoded SHA-256 of the bytes that were actually written
    /// to `models_dir`.
    pub actual_sha256: String,
    /// `true` iff the pack carried a non-empty canonical hash and
    /// it matched `actual_sha256`. `false` means the pack hadn't
    /// been pinned yet — the file *is* installed, but the registry
    /// can't confirm it's the canonical artefact.
    pub verified: bool,
    /// Size of the installed file in bytes (matches `models_dir/file_path`).
    pub size_bytes: u64,
}

/// Install an optional model pack from a user-provided source path.
///
/// The flow is intentionally *purely local*: KCreate does not reach
/// over the network to fetch weights itself (`local_first.rs`
/// deny-list enforces this). The user downloads the file out of
/// band — typically from `ModelPack::download_url` — and points the
/// installer at it.
///
/// Steps, in order:
/// 1. Resolve the pack id against the canonical registry. Built-in
///    packs and unknown ids are rejected.
/// 2. Stream the source file through SHA-256.
/// 3. If `pack.sha256` is non-empty, compare the hashes; on
///    mismatch, error *without writing*.
/// 4. Write the bytes to `models_dir/<file_path>.tmp` and then
///    atomically rename into place. This avoids leaving a
///    half-installed file if the process dies mid-write.
/// 5. Return an [`InstallReport`] with the actual hash and a
///    `verified` flag the UI can surface.
pub fn install_model_pack(
    pack_id: &str,
    source: &Path,
    models_dir: &Path,
) -> Result<InstallReport, InstallError> {
    let mut pack = static_packs()
        .into_iter()
        .find(|p| p.id == pack_id)
        .ok_or_else(|| InstallError::UnknownPack(pack_id.into()))?;
    if pack.kind == ModelKind::BuiltIn || pack.file_path.is_empty() {
        return Err(InstallError::BuiltIn(pack_id.into()));
    }
    // Overlay the canonical hash from the release-pinned table.
    // Without this, an unpinned pack would install as "unverified"
    // even when the publishing pipeline has pinned a hash — and a
    // hash-mismatching artefact for a pinned pack would silently
    // succeed.
    let canonical = canonical_hash_for(&pack.id);
    if !canonical.is_empty() {
        pack.sha256 = canonical.into();
    }

    // Stream the source through SHA-256 so we never have to hold the
    // whole multi-gigabyte file in memory. We also stage to a temp
    // file in `models_dir` and `rename()` into place at the end, so
    // a crash mid-copy never leaves a half-written
    // `models_dir/<file_path>` behind.
    fs::create_dir_all(models_dir).map_err(|e| InstallError::DestIo {
        path: models_dir.display().to_string(),
        source: e,
    })?;
    let dest = models_dir.join(&pack.file_path);
    let tmp = models_dir.join(format!("{}.tmp", &pack.file_path));

    let mut src_file = File::open(source).map_err(|e| InstallError::SourceIo {
        path: source.display().to_string(),
        source: e,
    })?;
    let mut tmp_file = File::create(&tmp).map_err(|e| InstallError::DestIo {
        path: tmp.display().to_string(),
        source: e,
    })?;
    let mut hasher = Sha256::new();
    // 64 KiB heap-allocated buffer keeps the stack frame tiny while
    // still amortising syscall overhead for multi-gigabyte weights.
    let mut buf: Box<[u8]> = vec![0u8; 64 * 1024].into_boxed_slice();
    let mut total: u64 = 0;
    loop {
        let n = src_file
            .read(&mut buf)
            .map_err(|e| InstallError::SourceIo {
                path: source.display().to_string(),
                source: e,
            })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        tmp_file
            .write_all(&buf[..n])
            .map_err(|e| InstallError::DestIo {
                path: tmp.display().to_string(),
                source: e,
            })?;
        total += n as u64;
    }
    tmp_file.sync_all().map_err(|e| InstallError::DestIo {
        path: tmp.display().to_string(),
        source: e,
    })?;
    drop(tmp_file);

    let actual_hex = hex_lower(&hasher.finalize());

    if !pack.sha256.is_empty() && pack.sha256 != actual_hex {
        // Hash mismatch: drop the temp file so we don't leak it.
        // We *don't* surface the rm error — the checksum mismatch is
        // the real failure the caller needs to know about.
        let _ = fs::remove_file(&tmp);
        return Err(InstallError::ChecksumMismatch {
            pack_id: pack_id.into(),
            expected: pack.sha256,
            actual: actual_hex,
        });
    }

    atomic_replace(&tmp, &dest).map_err(|e| InstallError::DestIo {
        path: dest.display().to_string(),
        source: e,
    })?;

    let verified = !pack.sha256.is_empty() && pack.sha256 == actual_hex;
    Ok(InstallReport {
        pack_id: pack_id.into(),
        actual_sha256: actual_hex,
        verified,
        size_bytes: total,
    })
}

/// Uninstall an optional model pack by deleting its file from
/// `models_dir`. Returns `Ok(())` even if the file was already
/// absent — that's the desired post-condition.
pub fn uninstall_model_pack(pack_id: &str, models_dir: &Path) -> Result<(), InstallError> {
    let pack = static_packs()
        .into_iter()
        .find(|p| p.id == pack_id)
        .ok_or_else(|| InstallError::UnknownPack(pack_id.into()))?;
    if pack.kind == ModelKind::BuiltIn || pack.file_path.is_empty() {
        return Err(InstallError::BuiltIn(pack_id.into()));
    }
    let dest = models_dir.join(&pack.file_path);
    match fs::remove_file(&dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(InstallError::DestIo {
            path: dest.display().to_string(),
            source: e,
        }),
    }
}

/// Atomically replace `dest` with `src` across Unix and Windows.
///
/// On Unix, [`fs::rename`] already performs an atomic replace if `dest`
/// exists. On Windows, the same call uses `MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING` since Rust 1.78, but it can still fail
/// with [`io::ErrorKind::AlreadyExists`] in edge cases — for example
/// when `dest` is held open by another process, or on older NTFS
/// configurations exposed via mounted SMB shares.
///
/// The bot's flagged scenario ("the `.tmp` was cleaned up but the dest
/// exists" on Windows during crash recovery) lands in exactly this
/// fallback path. We resolve it by removing `dest` and retrying the
/// rename. This is *not* atomic in the strict sense (there is a brief
/// window where `dest` is missing), but the alternative — leaving the
/// install in a broken state where the user can never re-install — is
/// worse than a sub-millisecond race that only matters if another
/// reader was already mid-load when the user clicked "install" on the
/// same pack a second time.
fn atomic_replace(src: &Path, dest: &Path) -> io::Result<()> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_file(dest)?;
            fs::rename(src, dest)
        }
        Err(e) => Err(e),
    }
}

/// Hex-encode a 32-byte SHA-256 digest as lowercase. Avoids pulling
/// in a separate `hex` crate.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_returns_full_set() {
        let dir = tempfile::tempdir().unwrap();
        let packs = list_model_packs(dir.path());
        // Lock the *full* canonical set of pack ids — every id in
        // `static_packs()` must appear here exactly once. A bare
        // `packs.len() == N` check would let a rename slip through
        // (count stays the same, id silently changes), so we compare
        // the sorted id vector instead. Per Devin Review 3289537741.
        let mut got: Vec<String> = packs.into_iter().map(|p| p.id).collect();
        got.sort();
        // Phase 12 Block A: removed `image_gen_flux_klein_mlx`,
        // `vision_qwen25vl_7b_mlx`, `vision_smolvlm_256m_mlx`; added
        // `llm_bonsai_1_7b`, `llm_bonsai_4b`, `llm_bonsai_8b`.
        let expected: Vec<&str> = vec![
            "bg_remove_threshold",
            "bg_remove_u2net",
            "image_gen_bonsai_gemlite_4b",
            "image_gen_bonsai_mlx_4b",
            "image_gen_flux_klein_4b",
            "image_gen_sd15",
            "llm_bonsai_1_7b",
            "llm_bonsai_4b",
            "llm_bonsai_8b",
            "llm_sidecar_3b",
            "ocr_heuristic",
            "palette_kmeans",
            "screenshot_to_layout",
            "segment_sam",
            "smart_select_flood",
            "upscale_esrgan",
            "upscale_lanczos",
            "vision_qwen25vl_7b",
            "vision_qwen25vl_7b_mmproj",
            "vision_smolvlm2_256m",
            "vision_smolvlm2_256m_mmproj",
        ];
        assert_eq!(got, expected, "pack id set drifted from canonical list");
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
            raw.contains("\"downloadUrl\""),
            "expected camelCase wire key `downloadUrl`, got {raw}"
        );
        assert!(
            !raw.contains("\"size_bytes\""),
            "snake_case `size_bytes` must not leak onto the wire: {raw}"
        );
        assert!(
            !raw.contains("\"file_path\""),
            "snake_case `file_path` must not leak onto the wire: {raw}"
        );
        assert!(
            !raw.contains("\"download_url\""),
            "snake_case `download_url` must not leak onto the wire: {raw}"
        );
    }

    /// Wire-format lockstep: `InstallReport` is what the Rust bridge
    /// returns from `aiInstallModelPack` (also flows through the
    /// Phase C `onboarding.installRecommendedPack` IPC, which forwards
    /// the JSON unchanged after a Node-side `JSON.parse` /
    /// `JSON.stringify` round-trip). The TypeScript consumers in
    /// `apps/desktop/shared/scene.ts::OnboardingInstallReport` and
    /// `apps/desktop/main/src/onboardingDownloader.ts::OnboardingInstallReport`
    /// MUST read the camelCase keys (`packId`, `actualSha256`,
    /// `sizeBytes`) because that's what `#[serde(rename_all =
    /// "camelCase")]` emits. A previous iteration of the TS layer
    /// expected snake_case (`pack_id`, `actual_sha256`, `size_bytes`)
    /// which silently broke the one-click install flow — the
    /// validation always rejected the Rust-emitted payload because
    /// every snake_case lookup was `undefined`. This test pins the
    /// canonical wire shape so any future field rename has to touch
    /// both sides in lockstep.
    #[test]
    fn install_report_serialises_to_camelcase_wire_format() {
        let report = InstallReport {
            pack_id: "llm_bonsai_1_7b".into(),
            actual_sha256: "0".repeat(64),
            verified: true,
            size_bytes: 750_000_000,
        };
        let raw = serde_json::to_string(&report).unwrap();
        // Round-trip back to the same struct.
        let parsed: InstallReport = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, report);
        // Pin the wire-format key names.
        assert!(
            raw.contains("\"packId\""),
            "expected camelCase wire key `packId`, got {raw}"
        );
        assert!(
            raw.contains("\"actualSha256\""),
            "expected camelCase wire key `actualSha256`, got {raw}"
        );
        assert!(
            raw.contains("\"sizeBytes\""),
            "expected camelCase wire key `sizeBytes`, got {raw}"
        );
        assert!(
            raw.contains("\"verified\""),
            "expected wire key `verified` (no rename), got {raw}"
        );
        assert!(
            !raw.contains("\"pack_id\""),
            "snake_case `pack_id` must not leak onto the wire: {raw}"
        );
        assert!(
            !raw.contains("\"actual_sha256\""),
            "snake_case `actual_sha256` must not leak onto the wire: {raw}"
        );
        assert!(
            !raw.contains("\"size_bytes\""),
            "snake_case `size_bytes` must not leak onto the wire: {raw}"
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

    /// Built-in packs (algorithm shipped inside `kcreate_ai`) must
    /// not be installable from a file — they're already part of the
    /// binary. The installer rejects them with [`InstallError::BuiltIn`]
    /// rather than silently no-op'ing.
    #[test]
    fn install_rejects_builtin_packs() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("anything.bin");
        fs::write(&src, b"data").unwrap();
        let err = install_model_pack("bg_remove_threshold", &src, dir.path()).unwrap_err();
        assert!(matches!(err, InstallError::BuiltIn(_)));
    }

    /// An unknown pack id is rejected before any IO so a typo can't
    /// scribble random files into `models_dir`.
    #[test]
    fn install_rejects_unknown_pack_id() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("anything.bin");
        fs::write(&src, b"data").unwrap();
        let err = install_model_pack("does_not_exist", &src, dir.path()).unwrap_err();
        assert!(matches!(err, InstallError::UnknownPack(_)));
    }

    /// Round-trip: install a Phase-2 optional pack (canonical hash
    /// is currently empty — pinning happens via the publishing
    /// pipeline) from a temp source, verify it lands in
    /// `models_dir/<file_path>`, the report carries the correct
    /// actual SHA-256, and `verified: false` (since the registry
    /// hash is empty).
    #[test]
    fn install_unverified_pack_copies_and_records_hash() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("u2net-from-author.onnx");
        let payload = b"deterministic-onnx-bytes";
        fs::write(&src, payload).unwrap();

        let models = dir.path().join("models");
        let report = install_model_pack("bg_remove_u2net", &src, &models).unwrap();
        assert_eq!(report.pack_id, "bg_remove_u2net");
        assert_eq!(report.size_bytes, payload.len() as u64);
        // SHA-256 of `payload`, hand-computed once and pinned here so
        // a regression in the streaming hasher would be caught.
        assert_eq!(
            report.actual_sha256,
            // sha256("deterministic-onnx-bytes")
            "a941a7caeaf1652305a6be8bcab2bc1206894c72a1840b9915de8417c0444aa2",
            "actual_sha256 must equal the hex-encoded SHA-256 of the source"
        );
        // Registry currently ships empty `sha256` for this pack, so
        // the installer correctly flags this as unverified.
        assert!(!report.verified, "expected verified == false for unpinned");
        assert!(models.join("u2net.onnx").exists());
        // The temp staging file must have been renamed away.
        assert!(!models.join("u2net.onnx.tmp").exists());
    }

    /// When the registry carries a canonical hash and the source
    /// matches, the report comes back `verified: true`. We patch a
    /// hash into the same pack via a sibling helper so the test
    /// doesn't depend on the publishing pipeline.
    #[test]
    fn install_verified_pack_sets_verified_true() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("u2net.onnx");
        let payload = b"canonical-onnx-bytes";
        fs::write(&src, payload).unwrap();

        // SHA-256 of `payload`, hand-computed via `sha256sum`.
        let expected_hash = "6b3c04b8a9c593d001a8099f26575219f0e3777050dc54dcb90e16fbcfe611ba";
        let models = dir.path().join("models");
        let report =
            install_with_expected_hash("bg_remove_u2net", &src, &models, expected_hash).unwrap();
        assert!(report.verified);
        assert_eq!(report.actual_sha256, expected_hash);
    }

    /// A registry hash that doesn't match the source must error out
    /// and leave nothing behind in `models_dir` — neither the final
    /// file nor the `.tmp` staging artefact.
    #[test]
    fn install_checksum_mismatch_aborts_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("u2net.onnx");
        fs::write(&src, b"wrong-bytes").unwrap();
        let models = dir.path().join("models");
        let err = install_with_expected_hash(
            "bg_remove_u2net",
            &src,
            &models,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap_err();
        assert!(matches!(err, InstallError::ChecksumMismatch { .. }));
        assert!(!models.join("u2net.onnx").exists());
        assert!(!models.join("u2net.onnx.tmp").exists());
    }

    /// Re-installing on top of an existing file succeeds. On Unix
    /// this just exercises `fs::rename`'s atomic-replace path; on
    /// Windows it covers the fallback in [`atomic_replace`] for the
    /// case where `fs::rename` would otherwise return
    /// [`io::ErrorKind::AlreadyExists`]. Either way, the final byte
    /// content must be the *new* source, not the stale destination
    /// (Devin Review BUG / PR #7).
    #[test]
    fn install_replaces_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");

        let src_old = dir.path().join("u2net-old.onnx");
        fs::write(&src_old, b"stale-bytes").unwrap();
        install_model_pack("bg_remove_u2net", &src_old, &models).unwrap();
        assert_eq!(fs::read(models.join("u2net.onnx")).unwrap(), b"stale-bytes",);

        // Re-install with new bytes on top of the existing file.
        let src_new = dir.path().join("u2net-new.onnx");
        fs::write(&src_new, b"fresh-bytes").unwrap();
        let report = install_model_pack("bg_remove_u2net", &src_new, &models).unwrap();
        assert_eq!(report.size_bytes, b"fresh-bytes".len() as u64);
        assert_eq!(
            fs::read(models.join("u2net.onnx")).unwrap(),
            b"fresh-bytes",
            "re-install must overwrite the destination atomically"
        );
        assert!(!models.join("u2net.onnx.tmp").exists());
    }

    /// Uninstall: removes the installed file. Calling uninstall when
    /// the file is already gone is a no-op so re-running uninstall
    /// doesn't error.
    #[test]
    fn uninstall_removes_file_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        fs::create_dir_all(&models).unwrap();
        let file = models.join("u2net.onnx");
        fs::write(&file, b"present").unwrap();

        uninstall_model_pack("bg_remove_u2net", &models).unwrap();
        assert!(!file.exists());
        // Idempotent: second call is a no-op, not an error.
        uninstall_model_pack("bg_remove_u2net", &models).unwrap();
    }

    #[test]
    fn uninstall_rejects_builtin_packs() {
        let dir = tempfile::tempdir().unwrap();
        let err = uninstall_model_pack("bg_remove_threshold", dir.path()).unwrap_err();
        assert!(matches!(err, InstallError::BuiltIn(_)));
    }

    /// Helper that overrides a pack's expected SHA-256 to test the
    /// "verified" path without relying on the publishing pipeline
    /// having pinned a hash yet. Mirrors `install_model_pack` but
    /// substitutes the static pack's hash with `expected_hash`.
    fn install_with_expected_hash(
        pack_id: &str,
        source: &Path,
        models_dir: &Path,
        expected_hash: &str,
    ) -> Result<InstallReport, InstallError> {
        let mut pack = static_packs()
            .into_iter()
            .find(|p| p.id == pack_id)
            .ok_or_else(|| InstallError::UnknownPack(pack_id.into()))?;
        pack.sha256 = expected_hash.into();
        // Inline the streaming-hash + atomic-rename logic so the
        // public installer's behaviour is matched exactly.
        if pack.kind == ModelKind::BuiltIn || pack.file_path.is_empty() {
            return Err(InstallError::BuiltIn(pack_id.into()));
        }
        fs::create_dir_all(models_dir).map_err(|e| InstallError::DestIo {
            path: models_dir.display().to_string(),
            source: e,
        })?;
        let dest = models_dir.join(&pack.file_path);
        let tmp = models_dir.join(format!("{}.tmp", &pack.file_path));
        let mut src_file = File::open(source).map_err(|e| InstallError::SourceIo {
            path: source.display().to_string(),
            source: e,
        })?;
        let mut tmp_file = File::create(&tmp).map_err(|e| InstallError::DestIo {
            path: tmp.display().to_string(),
            source: e,
        })?;
        let mut hasher = Sha256::new();
        let mut buf: Box<[u8]> = vec![0u8; 64 * 1024].into_boxed_slice();
        let mut total: u64 = 0;
        loop {
            let n = src_file
                .read(&mut buf)
                .map_err(|e| InstallError::SourceIo {
                    path: source.display().to_string(),
                    source: e,
                })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            tmp_file
                .write_all(&buf[..n])
                .map_err(|e| InstallError::DestIo {
                    path: tmp.display().to_string(),
                    source: e,
                })?;
            total += n as u64;
        }
        drop(tmp_file);
        let actual_hex = hex_lower(&hasher.finalize());
        if pack.sha256 != actual_hex {
            let _ = fs::remove_file(&tmp);
            return Err(InstallError::ChecksumMismatch {
                pack_id: pack_id.into(),
                expected: pack.sha256,
                actual: actual_hex,
            });
        }
        fs::rename(&tmp, &dest).map_err(|e| InstallError::DestIo {
            path: dest.display().to_string(),
            source: e,
        })?;
        Ok(InstallReport {
            pack_id: pack_id.into(),
            actual_sha256: actual_hex,
            verified: true,
            size_bytes: total,
        })
    }

    /// CI guard for the canonical-hash overlay table.
    ///
    /// Every entry in [`CANONICAL_PACK_HASHES`] must reference a real
    /// pack id and supply a syntactically valid SHA-256 digest
    /// (exactly 64 lowercase hex characters). Stale ids or
    /// shorter/longer hashes are a fast path to a broken release:
    /// the installer would either silently skip verification or
    /// reject every artefact for that pack.
    #[test]
    fn canonical_pack_hashes_are_well_formed() {
        let valid_ids: std::collections::HashSet<String> =
            static_packs().into_iter().map(|p| p.id).collect();
        for (pack_id, hash) in CANONICAL_PACK_HASHES.iter().copied() {
            assert!(
                valid_ids.contains(pack_id),
                "canonical hash entry references unknown pack id `{pack_id}`"
            );
            assert_eq!(
                hash.len(),
                64,
                "canonical hash for `{pack_id}` must be 64 hex chars, got {}",
                hash.len()
            );
            assert!(
                hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
                "canonical hash for `{pack_id}` must be lowercase hex"
            );
        }
    }

    /// When a canonical hash is registered for a pack, `list_model_packs`
    /// surfaces it on the returned `ModelPack::sha256` so the UI can
    /// show "Verified" affordances without re-reading the registry.
    #[test]
    fn canonical_hash_for_lookup_is_overlaid_on_list() {
        // The overlay table is empty on `main`, so the production
        // lookup always returns "". This test pins that property and
        // also exercises the overlay function directly so a future
        // entry actually overrides the catalogue's empty string.
        let unpinned = canonical_hash_for("bg_remove_u2net");
        assert!(
            unpinned.is_empty(),
            "no canonical hashes ship on main; release branches add them"
        );
        let dir = tempfile::tempdir().unwrap();
        let packs = list_model_packs(dir.path());
        let u2net = packs
            .iter()
            .find(|p| p.id == "bg_remove_u2net")
            .expect("u2net is in static_packs");
        assert!(
            u2net.sha256.is_empty(),
            "unpinned pack must surface empty sha256 to the UI"
        );
    }

    // ---- Phase 4 model-registry completeness tests (Task 27) ----

    /// Every vision pack must declare the `vision` capability OR be
    /// the mmproj companion (capability marker `mmproj`). Catches a
    /// future contributor adding a vision model without the
    /// capability tag the dispatcher uses to enumerate VLMs.
    #[test]
    fn vision_packs_declare_vision_or_mmproj_capability() {
        let dir = tempfile::tempdir().unwrap();
        for p in list_model_packs(dir.path())
            .into_iter()
            .filter(|p| p.category == ModelPackCategory::Vision)
        {
            assert!(
                p.capabilities
                    .iter()
                    .any(|c| c == "vision" || c == "mmproj"),
                "vision-category pack {} must declare `vision` or `mmproj`",
                p.id,
            );
        }
    }

    /// Generation packs must declare the `image_generation` capability
    /// so the diffusion sidecar selector can find them.
    #[test]
    fn generation_packs_declare_image_generation_capability() {
        let dir = tempfile::tempdir().unwrap();
        for p in list_model_packs(dir.path())
            .into_iter()
            .filter(|p| p.category == ModelPackCategory::Generation)
        {
            assert!(
                p.capabilities.iter().any(|c| c == "image_generation"),
                "generation pack {} must declare `image_generation`",
                p.id,
            );
        }
    }

    /// `mmproj_for` must resolve to a pack id that exists in
    /// `static_packs()`. Otherwise the sidecar dispatcher would
    /// silently fail to find the projector file.
    #[test]
    fn mmproj_for_targets_resolve_to_real_packs() {
        let dir = tempfile::tempdir().unwrap();
        let ids: Vec<String> = list_model_packs(dir.path())
            .into_iter()
            .map(|p| p.id)
            .collect();
        for parent in ["vision_smolvlm2_256m", "vision_qwen25vl_7b"] {
            let mmproj_id =
                mmproj_for(parent).unwrap_or_else(|| panic!("mmproj_for({parent}) returned None"));
            assert!(
                ids.iter().any(|i| i == mmproj_id),
                "mmproj pack id {mmproj_id} (for {parent}) is not in static_packs",
            );
        }
    }

    /// Tier-pack size policy: every vision pack must fit within
    /// `vision_model_max_mb` for the tier that recommends it. If
    /// `recommended_vision_pack` returns a pack that exceeds the
    /// tier ceiling, the install button would always be disabled —
    /// a UX regression worth catching at compile-test time.
    #[test]
    fn recommended_vision_pack_fits_under_tier_cap() {
        use kcreate_core::config::{DeviceTier, Platform};
        let dir = tempfile::tempdir().unwrap();
        let packs = list_model_packs(dir.path());
        for tier in [
            DeviceTier::Tier0,
            DeviceTier::Tier1,
            DeviceTier::Tier2,
            DeviceTier::Tier3,
        ] {
            for platform in [Platform::LinuxX64, Platform::MacOsAppleSilicon] {
                let Some(id) = recommended_vision_pack(tier, platform) else {
                    continue;
                };
                let pack = packs
                    .iter()
                    .find(|p| p.id == id)
                    .unwrap_or_else(|| panic!("recommended pack {id} not in registry"));
                let pack_mb = pack.size_bytes / (1024 * 1024);
                let cap_mb = tier.vision_model_max_mb();
                assert!(
                    pack_mb <= cap_mb,
                    "{id} ({pack_mb} MB) exceeds {tier:?} cap {cap_mb} MB on {platform:?}",
                );
            }
        }
    }

    /// `recommended_generation_pack` must return `None` for any
    /// tier that doesn't allow image generation. UI hard-gating
    /// depends on this — a regression would surface generation
    /// affordances on Tier 0 / 1 machines.
    #[test]
    fn recommended_generation_pack_respects_hard_gate() {
        use kcreate_core::config::{DeviceTier, Platform};
        assert_eq!(
            recommended_generation_pack(DeviceTier::Tier0, Platform::LinuxX64),
            None
        );
        assert_eq!(
            recommended_generation_pack(DeviceTier::Tier1, Platform::LinuxX64),
            None
        );
        assert_eq!(
            recommended_generation_pack(DeviceTier::Tier1, Platform::MacOsAppleSilicon),
            None
        );
        assert!(recommended_generation_pack(DeviceTier::Tier2, Platform::LinuxX64).is_some());
        assert!(recommended_generation_pack(DeviceTier::Tier3, Platform::LinuxX64).is_some());
    }

    /// The generation recommendation is tier-aware: the Tier 2 floor
    /// gets the small, fully-verified SD 1.5 checkpoint; Tier 3 gets
    /// the larger FLUX standalone model. Both recommendations must
    /// resolve to real packs in the catalogue.
    #[test]
    fn recommended_generation_pack_is_tier_aware() {
        use kcreate_core::config::{DeviceTier, Platform};
        assert_eq!(
            recommended_generation_pack(DeviceTier::Tier2, Platform::LinuxX64),
            Some("image_gen_sd15"),
        );
        assert_eq!(
            recommended_generation_pack(DeviceTier::Tier3, Platform::LinuxX64),
            Some("image_gen_flux_klein_4b"),
        );
        let dir = tempfile::tempdir().unwrap();
        let ids: std::collections::HashSet<String> = list_model_packs(dir.path())
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert!(ids.contains("image_gen_sd15"));
        assert!(ids.contains("image_gen_flux_klein_4b"));
    }

    /// `generation_pack_is_fused_checkpoint` selects the sd-server
    /// load flag in the bridge. SD 1.5 is a fused checkpoint (`-m`);
    /// FLUX is a standalone diffusion model (`--diffusion-model`).
    /// An unknown pack id must default to standalone (`false`) so a
    /// future pack can't accidentally inherit the `-m` path.
    #[test]
    fn generation_pack_fused_classification() {
        assert!(generation_pack_is_fused_checkpoint("image_gen_sd15"));
        assert!(!generation_pack_is_fused_checkpoint(
            "image_gen_flux_klein_4b"
        ));
        assert!(!generation_pack_is_fused_checkpoint("not_a_real_pack"));
    }

    /// The two Bonsai Image Ternary 4B variants must be real,
    /// downloadable, checksum-pinned `Generation` packs sitting next to
    /// SD 1.5 in the registry, each carrying the `image_generation`
    /// capability and a primary-transformer `file_path`. They are NOT
    /// fused checkpoints — they route to the Bonsai runner, not
    /// sd-server's `-m` path.
    #[test]
    fn bonsai_image_packs_are_real_and_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let packs = list_model_packs(dir.path());
        for (id, primary_ext, want_hash) in [
            (
                "image_gen_bonsai_mlx_4b",
                ".safetensors",
                "b21737bdf02690b7d662907781c4dc8b8bf22a2c98b823b1ca3336f48371a84f",
            ),
            (
                "image_gen_bonsai_gemlite_4b",
                ".pt",
                "a3a7df8a90374fea24afce3b36f00b4c728d0254717143d61f912a7b3070e7ac",
            ),
        ] {
            let pack = packs
                .iter()
                .find(|p| p.id == id)
                .unwrap_or_else(|| panic!("{id} is in static_packs"));
            assert_eq!(pack.category, ModelPackCategory::Generation);
            assert_eq!(pack.kind, ModelKind::Sidecar);
            assert!(pack.capabilities.iter().any(|c| c == "image_generation"));
            assert!(
                pack.download_url
                    .starts_with("https://huggingface.co/prism-ml/bonsai-image-ternary-4B-"),
                "{id} must download from the upstream prism-ml repo, got {}",
                pack.download_url,
            );
            assert!(
                pack.file_path.ends_with(primary_ext) && !pack.file_path.contains('/'),
                "{id} file_path must be a flat {primary_ext} filename, got {}",
                pack.file_path,
            );
            assert!(pack.size_bytes > 1_000_000_000);
            // Pinned to the upstream LFS oid so the installer verifies
            // the primary transformer download.
            assert_eq!(pack.sha256, want_hash);
            assert_eq!(canonical_hash_for(id), pack.sha256);
            // Bonsai is never a fused sd-server checkpoint.
            assert!(!generation_pack_is_fused_checkpoint(id));
        }
    }

    /// Engine routing: only the two Bonsai pack ids select a Bonsai
    /// runner; everything else (SD 1.5, FLUX, unknown ids) stays on
    /// sd-server. This is what keeps SD 1.5 the universal fallback.
    #[test]
    fn generation_engine_routing() {
        assert_eq!(
            generation_engine_for("image_gen_bonsai_mlx_4b"),
            GenerationEngine::BonsaiMlx,
        );
        assert_eq!(
            generation_engine_for("image_gen_bonsai_gemlite_4b"),
            GenerationEngine::BonsaiGemlite,
        );
        assert_eq!(
            generation_engine_for("image_gen_sd15"),
            GenerationEngine::SdCpp,
        );
        assert_eq!(
            generation_engine_for("image_gen_flux_klein_4b"),
            GenerationEngine::SdCpp,
        );
        assert_eq!(
            generation_engine_for("not_a_real_pack"),
            GenerationEngine::SdCpp,
        );
    }

    /// Engine ⇄ platform compatibility and the platform → Bonsai
    /// variant mapping must agree: each Bonsai variant only supports
    /// the platform that selects it, sd-server supports every
    /// platform, and Intel macOS / ARM Linux have no Bonsai variant.
    #[test]
    fn bonsai_engine_platform_compatibility() {
        use kcreate_core::config::Platform;

        // sd-server is universal.
        for platform in [
            Platform::MacOsIntel,
            Platform::MacOsAppleSilicon,
            Platform::WindowsX64,
            Platform::LinuxX64,
            Platform::LinuxArm64,
        ] {
            assert!(GenerationEngine::SdCpp.supports_platform(platform));
        }

        // MLX → Apple Silicon only.
        assert!(GenerationEngine::BonsaiMlx.supports_platform(Platform::MacOsAppleSilicon));
        for platform in [
            Platform::MacOsIntel,
            Platform::WindowsX64,
            Platform::LinuxX64,
            Platform::LinuxArm64,
        ] {
            assert!(!GenerationEngine::BonsaiMlx.supports_platform(platform));
        }

        // GemLite → CUDA on Windows / x86-64 Linux.
        for platform in [Platform::WindowsX64, Platform::LinuxX64] {
            assert!(GenerationEngine::BonsaiGemlite.supports_platform(platform));
        }
        for platform in [
            Platform::MacOsIntel,
            Platform::MacOsAppleSilicon,
            Platform::LinuxArm64,
        ] {
            assert!(!GenerationEngine::BonsaiGemlite.supports_platform(platform));
        }

        // The platform → variant map must point at a pack the engine
        // for that pack actually supports on that platform.
        for platform in [
            Platform::MacOsIntel,
            Platform::MacOsAppleSilicon,
            Platform::WindowsX64,
            Platform::LinuxX64,
            Platform::LinuxArm64,
        ] {
            match bonsai_image_variant_for_platform(platform) {
                Some(id) => {
                    assert!(generation_engine_for(id).supports_platform(platform));
                    // Selector must never point at a non-Bonsai pack.
                    assert_ne!(generation_engine_for(id), GenerationEngine::SdCpp);
                }
                None => assert!(matches!(
                    platform,
                    Platform::MacOsIntel | Platform::LinuxArm64
                )),
            }
        }
    }

    /// The engine wire strings are the stable contract the status IPC
    /// and `apps/desktop/shared/scene.ts` depend on. Freeze them.
    #[test]
    fn generation_engine_wire_strings() {
        assert_eq!(GenerationEngine::SdCpp.as_wire_str(), "sd_cpp");
        assert_eq!(GenerationEngine::BonsaiMlx.as_wire_str(), "bonsai_mlx");
        assert_eq!(
            GenerationEngine::BonsaiGemlite.as_wire_str(),
            "bonsai_gemlite",
        );
    }

    /// The SD 1.5 generation pack must be a real, downloadable,
    /// checksum-pinned `Generation` pack: it carries a download URL,
    /// the `image_generation` capability, and a canonical SHA-256 the
    /// installer enforces. This is the pack used for the end-to-end
    /// download + verify + run proof, so a regression here breaks the
    /// only fully-exercised generation path.
    #[test]
    fn sd15_pack_is_real_and_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let pack = list_model_packs(dir.path())
            .into_iter()
            .find(|p| p.id == "image_gen_sd15")
            .expect("image_gen_sd15 is in static_packs");
        assert_eq!(pack.category, ModelPackCategory::Generation);
        assert_eq!(pack.kind, ModelKind::Sidecar);
        assert!(pack.capabilities.iter().any(|c| c == "image_generation"));
        assert!(pack.download_url.starts_with("https://huggingface.co/"));
        assert!(!pack.file_path.is_empty());
        assert!(pack.size_bytes > 1_000_000_000);
        // The canonical hash is overlaid onto the wire pack so the UI
        // can show a "Verified" affordance, and the installer enforces
        // it (unlike the unpinned FLUX pack which still ships "").
        assert_eq!(pack.sha256.len(), 64);
        assert_eq!(canonical_hash_for("image_gen_sd15"), pack.sha256);
    }

    /// Phase 12 Block A invariant: no recommendation \u2014 vision,
    /// generation, or LLM \u2014 may return an MLX pack on any platform,
    /// because the registry no longer contains MLX packs. Any future
    /// `_mlx` pack id that slips into the recommended set would be a
    /// silent regression of the Python-elimination work.
    #[test]
    fn no_recommendation_is_mlx_after_phase12() {
        use kcreate_core::config::{DeviceTier, Platform};
        for tier in [
            DeviceTier::Tier0,
            DeviceTier::Tier1,
            DeviceTier::Tier2,
            DeviceTier::Tier3,
        ] {
            for platform in [
                Platform::LinuxX64,
                Platform::WindowsX64,
                Platform::MacOsIntel,
                Platform::MacOsAppleSilicon,
            ] {
                if let Some(v) = recommended_vision_pack(tier, platform) {
                    assert!(
                        !v.ends_with("_mlx"),
                        "vision recommendation must not be MLX after Phase 12: tier={tier:?} platform={platform:?} got {v}",
                    );
                }
                if let Some(g) = recommended_generation_pack(tier, platform) {
                    assert!(
                        !g.ends_with("_mlx"),
                        "generation recommendation must not be MLX after Phase 12: tier={tier:?} platform={platform:?} got {g}",
                    );
                }
                if let Some(l) = recommended_llm_pack(tier, platform) {
                    assert!(
                        !l.ends_with("_mlx"),
                        "LLM recommendation must not be MLX after Phase 12: tier={tier:?} platform={platform:?} got {l}",
                    );
                }
            }
        }
    }

    /// Phase 12 Block A: the LLM recommendation must map each tier
    /// to its tier-aware Ternary-Bonsai GGUF pack.
    #[test]
    fn recommended_llm_pack_is_bonsai_per_tier() {
        use kcreate_core::config::{DeviceTier, Platform};
        for platform in [
            Platform::LinuxX64,
            Platform::WindowsX64,
            Platform::MacOsIntel,
            Platform::MacOsAppleSilicon,
        ] {
            assert_eq!(
                recommended_llm_pack(DeviceTier::Tier0, platform),
                Some("llm_bonsai_1_7b"),
                "Tier 0 should recommend Ternary-Bonsai 1.7B on {platform:?}",
            );
            assert_eq!(
                recommended_llm_pack(DeviceTier::Tier1, platform),
                Some("llm_bonsai_4b"),
                "Tier 1 should recommend Ternary-Bonsai 4B on {platform:?}",
            );
            assert_eq!(
                recommended_llm_pack(DeviceTier::Tier2, platform),
                Some("llm_bonsai_8b"),
                "Tier 2 should recommend Ternary-Bonsai 8B on {platform:?}",
            );
            assert_eq!(
                recommended_llm_pack(DeviceTier::Tier3, platform),
                Some("llm_bonsai_8b"),
                "Tier 3 should recommend Ternary-Bonsai 8B on {platform:?}",
            );
        }
    }

    /// `ModelPackCategory` wire format must stay in sync with
    /// `apps/desktop/shared/scene.ts`'s union. Every variant has to
    /// serialize to the snake_case string the TS layer expects.
    #[test]
    fn category_serde_matches_typescript_wire_format() {
        assert_eq!(
            serde_json::to_string(&ModelPackCategory::Vision).unwrap(),
            "\"vision\""
        );
        assert_eq!(
            serde_json::to_string(&ModelPackCategory::Generation).unwrap(),
            "\"generation\""
        );
        assert_eq!(
            serde_json::to_string(&ModelPackCategory::Core).unwrap(),
            "\"core\""
        );
        assert_eq!(
            serde_json::to_string(&ModelPackCategory::ImagePro).unwrap(),
            "\"image_pro\""
        );
        assert_eq!(
            serde_json::to_string(&ModelPackCategory::DesignPro).unwrap(),
            "\"design_pro\""
        );
    }

    /// Phase 12 migration table: legacy MLX pack ids saved in
    /// Phase 4–11 project files must rewrite to their current GGUF
    /// equivalents. Without this, a user upgrading from a Phase 11
    /// install would see an opaque `ModelMissing` error on the
    /// first launch.
    #[test]
    fn migrate_legacy_pack_id_rewrites_known_mlx_ids() {
        assert_eq!(
            migrate_legacy_pack_id("vision_smolvlm_256m_mlx"),
            Some("vision_smolvlm2_256m"),
        );
        assert_eq!(
            migrate_legacy_pack_id("vision_qwen25vl_7b_mlx"),
            Some("vision_qwen25vl_7b"),
        );
        assert_eq!(
            migrate_legacy_pack_id("image_gen_flux_klein_mlx"),
            Some("image_gen_flux_klein_4b"),
        );
    }

    /// Migration is a no-op for already-current ids — callers
    /// distinguish "no rewrite needed" from "unknown pack" by the
    /// `None` return.
    #[test]
    fn migrate_legacy_pack_id_passes_through_current_ids() {
        for id in [
            "llm_bonsai_1_7b",
            "llm_bonsai_4b",
            "llm_bonsai_8b",
            "llm_sidecar_3b",
            "vision_smolvlm2_256m",
            "vision_qwen25vl_7b",
            "image_gen_flux_klein_4b",
            "bg_remove_u2net",
        ] {
            assert_eq!(
                migrate_legacy_pack_id(id),
                None,
                "current id `{id}` must NOT be rewritten",
            );
        }
    }

    /// Unknown ids surface as `None` too — the migration helper is
    /// scoped to legacy MLX ids only; surfacing a typo or stray
    /// string as `ModelMissing` later is the desired behavior.
    #[test]
    fn migrate_legacy_pack_id_passes_through_unknown_ids() {
        assert_eq!(migrate_legacy_pack_id(""), None);
        assert_eq!(migrate_legacy_pack_id("totally_not_a_pack"), None);
        assert_eq!(migrate_legacy_pack_id("vision_qwen25vl_7b_mlx_typo"), None);
    }

    /// Every migration target must be a real pack id in the
    /// current registry. Catches the regression where someone
    /// renames a pack but forgets to update the migration table.
    #[test]
    fn migrate_legacy_pack_id_targets_exist_in_registry() {
        let dir = tempfile::tempdir().unwrap();
        let current_ids: Vec<String> = list_model_packs(dir.path())
            .into_iter()
            .map(|p| p.id)
            .collect();
        for legacy in [
            "vision_smolvlm_256m_mlx",
            "vision_qwen25vl_7b_mlx",
            "image_gen_flux_klein_mlx",
        ] {
            let target = migrate_legacy_pack_id(legacy)
                .unwrap_or_else(|| panic!("legacy id `{legacy}` must migrate"));
            assert!(
                current_ids.iter().any(|id| id == target),
                "migration target `{target}` for legacy id `{legacy}` is not a registry pack",
            );
        }
    }
}
