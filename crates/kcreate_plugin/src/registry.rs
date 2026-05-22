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

use crate::manifest::{ManifestError, PluginManifest};

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
}

impl PluginRegistry {
    /// Create a registry rooted at `plugin_dir`. Does not scan — call
    /// [`Self::scan`] to populate.
    pub fn new(plugin_dir: PathBuf) -> Self {
        Self {
            plugin_dir,
            plugins: HashMap::new(),
            enabled: HashMap::new(),
            scan_cache: None,
        }
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
            match PluginManifest::load(&dir) {
                Ok(m) => {
                    let manifest_path = dir.join("manifest.json");
                    if let Ok(meta) = std::fs::metadata(&manifest_path) {
                        if let Ok(mtime) = meta.modified() {
                            manifest_mtimes.insert(manifest_path, mtime);
                        }
                    }
                    self.plugins.insert(
                        m.id.clone(),
                        PluginRecord {
                            dir: dir.clone(),
                            manifest: m,
                        },
                    );
                }
                Err(e) => log::warn!("kcreate_plugin: skipping {} ({e})", dir.display()),
            }
        }
        self.load_enabled()?;
        self.refresh_scan_cache(manifest_mtimes);
        Ok(())
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
}
