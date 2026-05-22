//! Plugin registry.
//!
//! Scans a directory of plugins (each in its own subdirectory with a
//! `manifest.json`) and maintains an in-memory map of `id -> manifest`
//! plus a flag for "enabled / disabled". Enable/disable state is
//! persisted to a small JSON file (`enabled.json`) inside `plugin_dir`
//! so it survives process restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Plugin registry. Owns the canonical map of installed plugins and
/// the on-disk enable/disable file.
#[derive(Debug)]
pub struct PluginRegistry {
    plugin_dir: PathBuf,
    plugins: HashMap<String, PluginManifest>,
    enabled: HashMap<String, bool>,
}

impl PluginRegistry {
    /// Create a registry rooted at `plugin_dir`. Does not scan — call
    /// [`Self::scan`] to populate.
    pub fn new(plugin_dir: PathBuf) -> Self {
        Self {
            plugin_dir,
            plugins: HashMap::new(),
            enabled: HashMap::new(),
        }
    }

    /// Scan `plugin_dir` for subdirectories containing a
    /// `manifest.json`. Plugins with invalid manifests are skipped
    /// (with a `log::warn!`) so a single bad plugin can't break the
    /// rest of the registry.
    pub fn scan(&mut self) -> Result<(), RegistryError> {
        self.plugins.clear();
        if !self.plugin_dir.exists() {
            std::fs::create_dir_all(&self.plugin_dir)?;
            // Empty dir means no plugins; still load (empty) enable file.
            self.load_enabled()?;
            return Ok(());
        }
        for entry in std::fs::read_dir(&self.plugin_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir = entry.path();
            match PluginManifest::load(&dir) {
                Ok(m) => {
                    self.plugins.insert(m.id.clone(), m);
                }
                Err(e) => log::warn!(
                    "kcreate_plugin: skipping {} ({e})",
                    dir.display()
                ),
            }
        }
        self.load_enabled()?;
        Ok(())
    }

    /// All known plugins (any state).
    pub fn list(&self) -> Vec<&PluginManifest> {
        let mut v: Vec<&PluginManifest> = self.plugins.values().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn get(&self, id: &str) -> Option<&PluginManifest> {
        self.plugins.get(id)
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
    pub fn entry_point_for(&self, id: &str) -> Option<PathBuf> {
        self.plugins
            .get(id)
            .map(|m| self.plugin_dir.join(&m.id).join(&m.entry_point))
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
}
