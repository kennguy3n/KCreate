//! Re-sign the bundled demo-plugin manifests with KCreate's
//! deterministic development signing key.
//!
//! Usage: `kcreate_sign_bundled <bundled_dir>`
//!
//! For every immediate subdirectory of `<bundled_dir>` that contains a
//! `manifest.json`, this writes a `manifest.json.sig` sidecar holding an
//! Ed25519 signature over the **exact bytes** of `manifest.json` (the
//! same verbatim-bytes contract `kcreate_plugin::trust` verifies). It
//! also (re)writes `<bundled_dir>/trusted_keys.json` carrying the public
//! half of the signing key so the host can seed its trust store.
//!
//! The signing key is derived from a fixed 32-byte seed — this is a
//! development key, not a production release key. It exists so the
//! shipped demo plugins carry a genuine, verifiable signature that the
//! in-app trust UX renders as "verified", and so anyone can regenerate
//! the signatures byte-for-byte after editing a manifest.

use std::path::{Path, PathBuf};

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;

/// Fixed development seed. 32 ASCII bytes — see module docs.
const SIGNING_SEED: &[u8; 32] = b"kcreate.demo.plugins.signing.k01";
/// Stable key id the manifests' sidecars reference.
const KEY_ID: &str = "com.kcreate.demos";

fn encode_b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bundled_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: kcreate_sign_bundled <bundled_dir>")?;
    if !bundled_dir.is_dir() {
        return Err(format!("bundled dir does not exist: {}", bundled_dir.display()).into());
    }

    let signing_key = SigningKey::from_bytes(SIGNING_SEED);
    let public_key_b64 = encode_b64(signing_key.verifying_key().as_bytes());

    // Trust store the host seeds at startup.
    let trusted_keys = json!([{
        "id": KEY_ID,
        "public_key_b64": public_key_b64,
        "comment": "KCreate bundled demo plugins (development signing key)"
    }]);
    let trusted_path = bundled_dir.join("trusted_keys.json");
    std::fs::write(&trusted_path, serde_json::to_vec_pretty(&trusted_keys)?)?;
    println!("wrote {}", trusted_path.display());

    // Sign every manifest.json found one level down.
    let mut signed = 0usize;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&bundled_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        sign_manifest(&signing_key, &dir, &manifest_path)?;
        signed += 1;
    }
    if signed == 0 {
        return Err("no manifest.json found under bundled dir".into());
    }
    println!("signed {signed} manifest(s) with key {KEY_ID}");
    Ok(())
}

fn sign_manifest(
    signing_key: &SigningKey,
    dir: &Path,
    manifest_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_bytes = std::fs::read(manifest_path)?;
    let signature = signing_key.sign(&manifest_bytes);
    let sidecar = json!({
        "key_id": KEY_ID,
        "signature_b64": encode_b64(&signature.to_bytes()),
    });
    let sig_path = dir.join("manifest.json.sig");
    std::fs::write(&sig_path, serde_json::to_vec_pretty(&sidecar)?)?;
    println!("signed {}", manifest_path.display());
    Ok(())
}
