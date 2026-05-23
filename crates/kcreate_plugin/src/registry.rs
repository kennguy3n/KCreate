//! Plugin registry.
//!
//! Scans a directory of plugins (each in its own subdirectory with a
//! `manifest.json`) and maintains an in-memory map of `id -> manifest`
//! plus a flag for "enabled / disabled". Enable/disable state is
//! persisted to a small JSON file (`enabled.json`) inside `plugin_dir`
//! so it survives process restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::manifest::{ManifestError, PluginManifest, PluginSignature};
use crate::trust::{TrustError, TrustStore};
use crate::PluginType;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest: {0}")]
    Manifest(#[from] ManifestError),
    #[error("plugin not found: {0}")]
    NotFound(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("trust: {0}")]
    Trust(#[from] TrustError),
    #[error("plugin {0}: native plugins must carry a manifest.json.sig signed by a trusted key")]
    UnsignedNativePlugin(String),
}

/// How a plugin's optional `manifest.json.sig` resolved against the
/// host's trust store.
///
/// The variants are deliberately split: a `Wasm` plugin with no
/// sidecar is fine (`Unsigned`), but a `Native` plugin with no
/// sidecar is rejected at load time by [`PluginRegistry::full_scan`]
/// rather than being recorded as `Unsigned` and surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SignatureStatus {
    /// No `manifest.json.sig` file alongside the manifest.
    Unsigned,
    /// Sidecar present and verified against a trusted key.
    Verified { key_id: String },
    /// Sidecar present but verification failed. Stored only for
    /// non-native plugins; native plugins with this status are
    /// rejected at scan time.
    Invalid { key_id: String, reason: String },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EnabledFile {
    enabled: Vec<String>,
}

/// One entry in the [`PluginRegistry`] map.
///
/// We track the *actual filesystem directory* the manifest was
/// loaded from in addition to the parsed manifest itself, because
/// plugin authors can choose any subdirectory name — we don't force
/// `<plugin_dir>/<manifest.id>/`. Reconstructing the path from
/// `manifest.id` would silently break when the two diverge (manifest
/// id `cool_plugin` shipped in a directory called `my-cool-plugin/`),
/// causing [`PluginRegistry::entry_point_for`] to point at a
/// non-existent file even though [`PluginRegistry::scan`] succeeded.
/// Per Devin Review BUG_pr-review-job-e31d5461e1ff4359ad80d927af5d0b54_0001.
#[derive(Debug)]
struct PluginRecord {
    /// Directory on disk this plugin was loaded from (parent of
    /// `manifest.json`). Always a child of
    /// [`PluginRegistry::plugin_dir`].
    dir: PathBuf,
    manifest: PluginManifest,
    /// How the optional `manifest.json.sig` sidecar resolved against
    /// the registry's [`TrustStore`] during the last scan.
    signature: SignatureStatus,
}

/// Snapshot of filesystem mtimes captured at the end of a successful
/// [`PluginRegistry::scan`]. Used by [`PluginRegistry::scan`] to skip
/// the directory walk + manifest re-parse when nothing on disk has
/// changed since the last scan. Per Devin Review
/// ANALYSIS_pr-review-job-0594c03f68c24589ba78a32926e3874f_0006.
///
/// `plugin_dir_mtime` changes when a plugin directory is added or
/// removed (POSIX directory mtime semantics). `manifest_mtimes`
/// catches edits to existing manifest files where the parent
/// directory's mtime does not change.
#[derive(Debug, Clone)]
struct ScanCache {
    plugin_dir_mtime: SystemTime,
    manifest_mtimes: HashMap<PathBuf, SystemTime>,
}

/// Plugin registry. Owns the canonical map of installed plugins and
/// the on-disk enable/disable file.
#[derive(Debug)]
pub struct PluginRegistry {
    plugin_dir: PathBuf,
    plugins: HashMap<String, PluginRecord>,
    enabled: HashMap<String, bool>,
    /// `None` until the first successful scan. After that, `scan`
    /// uses this to short-circuit if nothing on disk has changed.
    scan_cache: Option<ScanCache>,
    /// The set of trusted Ed25519 public keys plugins can be signed
    /// against. Defaults to empty, which is fine for hosts that only
    /// run WASM and JsPanel plugins — those types don't require
    /// signatures. Native plugins, however, require a non-empty
    /// trust store; loading them without one is rejected with
    /// [`RegistryError::UnsignedNativePlugin`].
    trust: TrustStore,
}

impl PluginRegistry {
    /// Create a registry rooted at `plugin_dir` with an empty trust
    /// store. Does not scan — call [`Self::scan`] to populate.
    pub fn new(plugin_dir: PathBuf) -> Self {
        Self::with_trust(plugin_dir, TrustStore::default())
    }

    /// Create a registry rooted at `plugin_dir` with the given trust
    /// store. Required for hosts that intend to load native plugins.
    pub fn with_trust(plugin_dir: PathBuf, trust: TrustStore) -> Self {
        Self {
            plugin_dir,
            plugins: HashMap::new(),
            enabled: HashMap::new(),
            scan_cache: None,
            trust,
        }
    }

    /// Swap the trust store and invalidate the scan cache so the
    /// next [`Self::scan`] re-evaluates every plugin's signature
    /// status. Useful when the host UI adds or removes a trusted
    /// key at runtime.
    pub fn set_trust_store(&mut self, trust: TrustStore) {
        self.trust = trust;
        self.scan_cache = None;
    }

    /// Snapshot of the trust store. Bridge callers use this to render
    /// "Trusted Authorities" in the UI.
    pub fn trust_store(&self) -> &TrustStore {
        &self.trust
    }

    /// Scan `plugin_dir` for subdirectories containing a
    /// `manifest.json`. Plugins with invalid manifests are skipped
    /// (with a `log::warn!`) so a single bad plugin can't break the
    /// rest of the registry.
    ///
    /// This is called on every PluginManager refresh / `plugin_list`
    /// bridge call. To keep that cheap even on slow filesystems we
    /// short-circuit when nothing on disk has changed since the
    /// previous successful scan, using mtime checks on both the
    /// plugin root and each individual `manifest.json`. The first
    /// scan, scans after the cache has been invalidated, and forced
    /// rescans (via [`Self::force_rescan`]) still pay the full walk
    /// cost. Per Devin Review
    /// ANALYSIS_pr-review-job-0594c03f68c24589ba78a32926e3874f_0006.
    pub fn scan(&mut self) -> Result<(), RegistryError> {
        if self.cache_is_fresh() {
            return Ok(());
        }
        self.full_scan()
    }

    /// Force a full rescan regardless of the mtime cache. Useful when
    /// the caller knows something about the filesystem the cache can't
    /// observe (e.g. an external plugin installer wrote files with
    /// `utimensat` and reset their mtimes).
    pub fn force_rescan(&mut self) -> Result<(), RegistryError> {
        self.scan_cache = None;
        self.full_scan()
    }

    /// Returns `true` when the cached scan is still valid against the
    /// current filesystem state — i.e. no further work is needed.
    fn cache_is_fresh(&self) -> bool {
        let Some(cache) = self.scan_cache.as_ref() else {
            return false;
        };
        let Ok(meta) = std::fs::metadata(&self.plugin_dir) else {
            // Plugin dir was removed underneath us — invalidate.
            return false;
        };
        let Ok(dir_mtime) = meta.modified() else {
            // Filesystem doesn't report mtimes (rare). Fall back to
            // always-rescan rather than miss changes.
            return false;
        };
        if dir_mtime != cache.plugin_dir_mtime {
            return false;
        }
        // Every cached manifest must still exist with the same mtime.
        for (path, expected) in &cache.manifest_mtimes {
            let Ok(mm) = std::fs::metadata(path) else {
                return false;
            };
            let Ok(seen) = mm.modified() else {
                return false;
            };
            if seen != *expected {
                return false;
            }
        }
        true
    }

    fn full_scan(&mut self) -> Result<(), RegistryError> {
        self.plugins.clear();
        if !self.plugin_dir.exists() {
            std::fs::create_dir_all(&self.plugin_dir)?;
            // Empty dir means no plugins; still load (empty) enable file.
            self.load_enabled()?;
            self.refresh_scan_cache(HashMap::new());
            return Ok(());
        }
        let mut manifest_mtimes: HashMap<PathBuf, SystemTime> = HashMap::new();
        for entry in std::fs::read_dir(&self.plugin_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir = entry.path();
            let (manifest, raw_bytes) = match PluginManifest::load_with_raw(&dir) {
                Ok(pair) => pair,
                Err(e) => {
                    log::warn!("kcreate_plugin: skipping {} ({e})", dir.display());
                    continue;
                }
            };
            let signature_status = match self.evaluate_signature(&dir, &raw_bytes) {
                Ok(status) => status,
                Err(e) => {
                    log::warn!(
                        "kcreate_plugin: skipping {} (signature sidecar error: {e})",
                        dir.display()
                    );
                    continue;
                }
            };
            // Native plugins MUST be verified. Anything else is a
            // load-time refusal — we never expose an unsigned or
            // invalidly-signed native plugin to the host or the UI.
            if manifest.plugin_type == PluginType::Native
                && !matches!(signature_status, SignatureStatus::Verified { .. })
            {
                log::warn!(
                    "kcreate_plugin: rejecting native plugin {} ({:?})",
                    manifest.id,
                    signature_status
                );
                continue;
            }
            let manifest_path = dir.join("manifest.json");
            if let Ok(meta) = std::fs::metadata(&manifest_path) {
                if let Ok(mtime) = meta.modified() {
                    manifest_mtimes.insert(manifest_path, mtime);
                }
            }
            self.plugins.insert(
                manifest.id.clone(),
                PluginRecord {
                    dir: dir.clone(),
                    manifest,
                    signature: signature_status,
                },
            );
        }
        self.load_enabled()?;
        self.refresh_scan_cache(manifest_mtimes);
        Ok(())
    }

    /// Evaluate the optional `manifest.json.sig` sidecar against the
    /// configured trust store. Returns:
    ///
    /// * `Ok(SignatureStatus::Unsigned)` if no sidecar exists.
    /// * `Ok(SignatureStatus::Verified)` if the sidecar verifies.
    /// * `Ok(SignatureStatus::Invalid)` if the sidecar exists and
    ///   parses but verification failed (wrong key id, malformed
    ///   signature, cryptographic mismatch). Callers may surface
    ///   this to the UI; the registry refuses to load native plugins
    ///   in this state.
    /// * `Err(ManifestError)` if the sidecar exists but is malformed
    ///   JSON or is missing required fields. We treat this as a load
    ///   failure rather than `Invalid` because the file was clearly
    ///   never produced by a working signer.
    fn evaluate_signature(
        &self,
        plugin_dir: &Path,
        manifest_bytes: &[u8],
    ) -> Result<SignatureStatus, ManifestError> {
        let Some(sidecar) = PluginSignature::load_optional(plugin_dir)? else {
            return Ok(SignatureStatus::Unsigned);
        };
        match self
            .trust
            .verify(&sidecar.key_id, manifest_bytes, &sidecar.signature_b64)
        {
            Ok(()) => Ok(SignatureStatus::Verified {
                key_id: sidecar.key_id,
            }),
            Err(e) => Ok(SignatureStatus::Invalid {
                key_id: sidecar.key_id,
                reason: e.to_string(),
            }),
        }
    }

    fn refresh_scan_cache(&mut self, manifest_mtimes: HashMap<PathBuf, SystemTime>) {
        // Re-stat plugin_dir *after* the walk so we capture its mtime
        // at the moment we know the snapshot is consistent.
        let Ok(meta) = std::fs::metadata(&self.plugin_dir) else {
            // Disappeared between scan and stat; drop the cache so
            // the next call rescans.
            self.scan_cache = None;
            return;
        };
        let plugin_dir_mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        self.scan_cache = Some(ScanCache {
            plugin_dir_mtime,
            manifest_mtimes,
        });
    }

    /// All known plugins (any state).
    pub fn list(&self) -> Vec<&PluginManifest> {
        let mut v: Vec<&PluginManifest> = self.plugins.values().map(|r| &r.manifest).collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn get(&self, id: &str) -> Option<&PluginManifest> {
        self.plugins.get(id).map(|r| &r.manifest)
    }

    /// Returns the signature status the most recent scan recorded for
    /// the given plugin id, or `None` if the id is unknown.
    pub fn signature_status_for(&self, id: &str) -> Option<&SignatureStatus> {
        self.plugins.get(id).map(|r| &r.signature)
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled.get(id).copied().unwrap_or(false)
    }

    pub fn enable(&mut self, id: &str) -> Result<(), RegistryError> {
        if !self.plugins.contains_key(id) {
            return Err(RegistryError::NotFound(id.to_string()));
        }
        self.enabled.insert(id.to_string(), true);
        self.persist_enabled()
    }

    pub fn disable(&mut self, id: &str) -> Result<(), RegistryError> {
        if !self.plugins.contains_key(id) {
            return Err(RegistryError::NotFound(id.to_string()));
        }
        self.enabled.insert(id.to_string(), false);
        self.persist_enabled()
    }

    pub fn plugin_dir(&self) -> &Path {
        &self.plugin_dir
    }

    /// Resolve the path to a plugin's entry-point file.
    ///
    /// Uses the directory the manifest was *actually loaded from*,
    /// not `plugin_dir.join(manifest.id)`. The two can differ when
    /// the plugin ships under a directory name that doesn't match its
    /// manifest id (e.g. `~/.kcreate/plugins/my-cool-plugin/` with a
    /// manifest declaring `"id": "cool_plugin"`).
    pub fn entry_point_for(&self, id: &str) -> Option<PathBuf> {
        self.plugins
            .get(id)
            .map(|r| r.dir.join(&r.manifest.entry_point))
    }

    fn enabled_path(&self) -> PathBuf {
        self.plugin_dir.join("enabled.json")
    }

    fn load_enabled(&mut self) -> Result<(), RegistryError> {
        let path = self.enabled_path();
        if !path.exists() {
            return Ok(());
        }
        let bytes = std::fs::read(&path)?;
        let file: EnabledFile = serde_json::from_slice(&bytes)?;
        for id in file.enabled {
            self.enabled.insert(id, true);
        }
        Ok(())
    }

    fn persist_enabled(&self) -> Result<(), RegistryError> {
        let file = EnabledFile {
            enabled: self
                .enabled
                .iter()
                .filter_map(|(k, v)| if *v { Some(k.clone()) } else { None })
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file)?;
        std::fs::write(self.enabled_path(), bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_plugin(parent: &Path, id: &str) {
        let dir = parent.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{
                    "id": "{id}",
                    "name": "{id}",
                    "version": "0.0.1",
                    "type": "wasm",
                    "entry_point": "p.wasm"
                }}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("p.wasm"), b"\0asm").unwrap();
    }

    #[test]
    fn scan_finds_all_plugins() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "alpha");
        write_plugin(dir.path(), "beta");
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        let ids: Vec<&str> = reg.list().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn enable_persists_across_scan() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "demo");
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        assert!(!reg.is_enabled("demo"));
        reg.enable("demo").unwrap();
        assert!(reg.is_enabled("demo"));

        // Re-scan with a fresh registry — enabled state should
        // survive.
        let mut reg2 = PluginRegistry::new(dir.path().to_path_buf());
        reg2.scan().unwrap();
        assert!(reg2.is_enabled("demo"));
    }

    #[test]
    fn disable_persists() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "demo");
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        reg.enable("demo").unwrap();
        reg.disable("demo").unwrap();
        let mut reg2 = PluginRegistry::new(dir.path().to_path_buf());
        reg2.scan().unwrap();
        assert!(!reg2.is_enabled("demo"));
    }

    #[test]
    fn enable_unknown_plugin_errors() {
        let dir = tempdir().unwrap();
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        assert!(matches!(
            reg.enable("nope"),
            Err(RegistryError::NotFound(_))
        ));
    }

    #[test]
    fn scan_skips_invalid_manifest() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "good");
        let bad = dir.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("manifest.json"), b"not json").unwrap();
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        let ids: Vec<&str> = reg.list().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["good"]);
    }

    #[test]
    fn entry_point_for_resolves_path() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "demo");
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        let p = reg.entry_point_for("demo").unwrap();
        assert!(p.ends_with("demo/p.wasm"));
    }

    /// Regression for Devin Review
    /// BUG_pr-review-job-e31d5461e1ff4359ad80d927af5d0b54_0001 — when the
    /// plugin's directory name differs from its manifest `id`,
    /// `entry_point_for` must still resolve to the *real* directory on
    /// disk, not `<plugin_dir>/<manifest.id>/...`.
    #[test]
    fn entry_point_for_uses_actual_directory_not_manifest_id() {
        let dir = tempdir().unwrap();
        // Directory is `my-cool-plugin/`, manifest declares `id =
        // "cool_plugin"` — a mismatch the old code couldn't survive.
        let plugin_dir = dir.path().join("my-cool-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{
                "id": "cool_plugin",
                "name": "Cool",
                "version": "0.0.1",
                "type": "wasm",
                "entry_point": "p.wasm"
            }"#,
        )
        .unwrap();
        std::fs::write(plugin_dir.join("p.wasm"), b"\0asm").unwrap();

        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        let resolved = reg.entry_point_for("cool_plugin").unwrap();
        assert_eq!(resolved, plugin_dir.join("p.wasm"));
        assert!(resolved.exists(), "resolved entry point must exist on disk");
    }

    /// Regression for Devin Review
    /// ANALYSIS_pr-review-job-0594c03f68c24589ba78a32926e3874f_0006 — when
    /// neither `plugin_dir` nor any manifest file has changed mtime,
    /// repeated `scan()` calls must short-circuit and return identical
    /// results without rebuilding the registry from scratch.
    ///
    /// The cache invariant is verified indirectly: we mutate a
    /// non-manifest file inside one of the plugin directories (which
    /// changes neither `plugin_dir`'s mtime nor any tracked manifest
    /// mtime) and confirm the registry's view is *exactly* what the
    /// first scan produced — including the in-memory plugin record
    /// PathBufs, which the cache short-circuit leaves intact.
    #[test]
    fn scan_short_circuits_on_unchanged_filesystem() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "alpha");
        write_plugin(dir.path(), "beta");
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        let ids_first: Vec<String> = reg.list().iter().map(|m| m.id.clone()).collect();
        let alpha_path_first = reg.entry_point_for("alpha").unwrap();

        // Modify a non-manifest file inside `alpha/`. POSIX directory
        // mtime semantics: writing to `alpha/p.wasm` changes neither
        // `<root>/`'s mtime nor `alpha/manifest.json`'s mtime — so the
        // cache should still be valid.
        std::fs::write(dir.path().join("alpha").join("p.wasm"), b"\0asm\0\0\0\0").unwrap();
        reg.scan().unwrap();
        let ids_after: Vec<String> = reg.list().iter().map(|m| m.id.clone()).collect();
        assert_eq!(
            ids_first, ids_after,
            "cached scan must return identical ids when mtimes are unchanged",
        );
        assert_eq!(reg.entry_point_for("alpha").unwrap(), alpha_path_first);
    }

    /// `force_rescan` must bypass the mtime cache and pick up real
    /// filesystem changes (plugin removal in this case).
    #[test]
    fn force_rescan_observes_filesystem_changes() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "alpha");
        write_plugin(dir.path(), "beta");
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        assert_eq!(reg.list().len(), 2);
        std::fs::remove_dir_all(dir.path().join("alpha")).unwrap();
        reg.force_rescan().unwrap();
        let ids: Vec<&str> = reg.list().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["beta"]);
    }

    /// A regular `scan()` (no force) MUST observe a manifest deletion
    /// via the manifest_mtimes invalidation path — the cache hit is
    /// an optimisation, not a stale-data trap.
    #[test]
    fn scan_picks_up_deleted_manifest() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "alpha");
        write_plugin(dir.path(), "beta");
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        std::fs::remove_file(dir.path().join("alpha").join("manifest.json")).unwrap();
        reg.scan().unwrap();
        let ids: Vec<&str> = reg.list().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["beta"]);
    }

    // ---------- Signature / native-plugin gating (Block E, Task 22) ----------
    //
    // The Native plugin runtime isn't shipped in Phase 2, but the
    // registry's *gate* is shipped now so the rest of the
    // installation pipeline (UI, bridge, install flow) can be built
    // on top of the invariant "if a record exists, native plugins
    // have a verified signature". The tests below pin that invariant.

    use ed25519_dalek::{Signer, SigningKey};

    use crate::trust::{encode_b64, TrustStore, TrustedKey};

    fn write_native_plugin_unsigned(parent: &Path, id: &str) {
        let dir = parent.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{
                    "id": "{id}",
                    "name": "{id}",
                    "version": "0.0.1",
                    "type": "native",
                    "entry_point": "p.bin"
                }}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("p.bin"), b"\x7fELF").unwrap();
    }

    fn write_native_plugin_signed(parent: &Path, id: &str, key_id: &str, sk: &SigningKey) {
        let dir = parent.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest_bytes = format!(
            r#"{{
                "id": "{id}",
                "name": "{id}",
                "version": "0.0.1",
                "type": "native",
                "entry_point": "p.bin"
            }}"#
        );
        std::fs::write(dir.join("manifest.json"), &manifest_bytes).unwrap();
        std::fs::write(dir.join("p.bin"), b"\x7fELF").unwrap();
        let sig = sk.sign(manifest_bytes.as_bytes());
        let sidecar = format!(
            r#"{{"key_id":"{key_id}","signature_b64":"{}"}}"#,
            encode_b64(&sig.to_bytes())
        );
        std::fs::write(dir.join("manifest.json.sig"), sidecar).unwrap();
    }

    fn trusted_store(key_id: &str, sk: &SigningKey) -> TrustStore {
        TrustStore::from_keys(vec![TrustedKey {
            id: key_id.to_string(),
            public_key_b64: encode_b64(sk.verifying_key().as_bytes()),
            comment: format!("test key {key_id}"),
        }])
        .unwrap()
    }

    #[test]
    fn unsigned_native_plugin_is_rejected_at_scan() {
        let dir = tempdir().unwrap();
        write_native_plugin_unsigned(dir.path(), "needs_sig");
        write_plugin(dir.path(), "wasm_ok"); // unsigned wasm is fine
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let mut reg =
            PluginRegistry::with_trust(dir.path().to_path_buf(), trusted_store("k1", &sk));
        reg.scan().unwrap();
        let ids: Vec<&str> = reg.list().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["wasm_ok"],
            "unsigned native plugin must not appear in the registry"
        );
    }

    #[test]
    fn signed_native_plugin_with_trusted_key_loads() {
        let dir = tempdir().unwrap();
        let sk = SigningKey::from_bytes(&[2u8; 32]);
        write_native_plugin_signed(dir.path(), "official", "k1", &sk);
        let mut reg =
            PluginRegistry::with_trust(dir.path().to_path_buf(), trusted_store("k1", &sk));
        reg.scan().unwrap();
        let ids: Vec<&str> = reg.list().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["official"]);
        match reg.signature_status_for("official").unwrap() {
            SignatureStatus::Verified { key_id } => assert_eq!(key_id, "k1"),
            other => panic!("expected Verified, got {other:?}"),
        }
    }

    #[test]
    fn signed_native_with_unknown_key_is_rejected() {
        let dir = tempdir().unwrap();
        let signer = SigningKey::from_bytes(&[3u8; 32]);
        let trusted = SigningKey::from_bytes(&[4u8; 32]);
        // Plugin signs with `signer` but trust store only knows `trusted`.
        write_native_plugin_signed(dir.path(), "evil", "k1", &signer);
        let mut reg =
            PluginRegistry::with_trust(dir.path().to_path_buf(), trusted_store("k1", &trusted));
        reg.scan().unwrap();
        assert!(
            reg.list().is_empty(),
            "native plugin signed by untrusted key must be rejected"
        );
    }

    #[test]
    fn tampered_native_manifest_is_rejected() {
        let dir = tempdir().unwrap();
        let sk = SigningKey::from_bytes(&[5u8; 32]);
        write_native_plugin_signed(dir.path(), "ok", "k1", &sk);
        // Tamper with the manifest *after* the signature was produced.
        let manifest_path = dir.path().join("ok").join("manifest.json");
        let bytes = std::fs::read(&manifest_path).unwrap();
        let tampered = String::from_utf8(bytes)
            .unwrap()
            .replace("\"ok\"", "\"hijacked\"");
        std::fs::write(&manifest_path, tampered).unwrap();
        let mut reg =
            PluginRegistry::with_trust(dir.path().to_path_buf(), trusted_store("k1", &sk));
        reg.scan().unwrap();
        assert!(
            reg.list().is_empty(),
            "manifest tampered after signing must not load"
        );
    }

    #[test]
    fn unsigned_wasm_plugin_loads_with_unsigned_status() {
        let dir = tempdir().unwrap();
        write_plugin(dir.path(), "wasm_plain");
        let mut reg = PluginRegistry::new(dir.path().to_path_buf());
        reg.scan().unwrap();
        assert_eq!(
            reg.signature_status_for("wasm_plain"),
            Some(&SignatureStatus::Unsigned),
        );
    }

    #[test]
    fn signed_wasm_plugin_records_verified_status() {
        let dir = tempdir().unwrap();
        let sk = SigningKey::from_bytes(&[6u8; 32]);
        // Hand-craft a signed WASM plugin (similar shape to
        // `write_native_plugin_signed` but type=wasm).
        let plugin_dir = dir.path().join("signed_wasm");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let manifest = r#"{
            "id": "signed_wasm",
            "name": "Signed WASM",
            "version": "0.0.1",
            "type": "wasm",
            "entry_point": "p.wasm"
        }"#;
        std::fs::write(plugin_dir.join("manifest.json"), manifest).unwrap();
        std::fs::write(plugin_dir.join("p.wasm"), b"\0asm").unwrap();
        let sig = sk.sign(manifest.as_bytes());
        std::fs::write(
            plugin_dir.join("manifest.json.sig"),
            format!(
                r#"{{"key_id":"kw","signature_b64":"{}"}}"#,
                encode_b64(&sig.to_bytes())
            ),
        )
        .unwrap();

        let mut reg =
            PluginRegistry::with_trust(dir.path().to_path_buf(), trusted_store("kw", &sk));
        reg.scan().unwrap();
        match reg.signature_status_for("signed_wasm").unwrap() {
            SignatureStatus::Verified { key_id } => assert_eq!(key_id, "kw"),
            other => panic!("expected Verified, got {other:?}"),
        }
    }

    #[test]
    fn signed_wasm_with_bad_signature_loads_as_invalid_not_rejected() {
        // Non-native plugins are allowed to load even with a broken
        // signature — the registry surfaces the status so the UI can
        // warn the user, but the plugin still runs (it's sandboxed).
        // Native plugins do not get this courtesy.
        let dir = tempdir().unwrap();
        let signer = SigningKey::from_bytes(&[7u8; 32]);
        let trusted = SigningKey::from_bytes(&[8u8; 32]);
        let plugin_dir = dir.path().join("wasm_with_bad_sig");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let manifest = r#"{
            "id": "wasm_with_bad_sig",
            "name": "Bad sig",
            "version": "0.0.1",
            "type": "wasm",
            "entry_point": "p.wasm"
        }"#;
        std::fs::write(plugin_dir.join("manifest.json"), manifest).unwrap();
        std::fs::write(plugin_dir.join("p.wasm"), b"\0asm").unwrap();
        let sig = signer.sign(manifest.as_bytes());
        std::fs::write(
            plugin_dir.join("manifest.json.sig"),
            format!(
                r#"{{"key_id":"k","signature_b64":"{}"}}"#,
                encode_b64(&sig.to_bytes())
            ),
        )
        .unwrap();
        let mut reg =
            PluginRegistry::with_trust(dir.path().to_path_buf(), trusted_store("k", &trusted));
        reg.scan().unwrap();
        match reg.signature_status_for("wasm_with_bad_sig").unwrap() {
            SignatureStatus::Invalid { key_id, .. } => assert_eq!(key_id, "k"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}
