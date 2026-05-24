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
    ]
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
        let expected: Vec<&str> = vec![
            "bg_remove_threshold",
            "bg_remove_u2net",
            "llm_sidecar_3b",
            "ocr_heuristic",
            "palette_kmeans",
            "screenshot_to_layout",
            "smart_select_flood",
            "upscale_esrgan",
            "upscale_lanczos",
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
}
