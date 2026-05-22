//! Plugin manifest parsing.
//!
//! A plugin directory contains a single `manifest.json` plus the
//! supporting files (`.wasm` for WASM plugins, JS bundles for panel
//! plugins, etc.). The manifest declares the plugin's identity,
//! version, and required permissions; the registry only loads plugins
//! whose manifest parses successfully.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What kind of plugin this manifest describes. The Phase 2 WASM
/// runtime only executes `Wasm` plugins; the other variants exist so
/// the registry can list them and the UI can communicate "this plugin
/// kind isn't supported yet".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginType {
    Wasm,
    JsPanel,
    Native,
}

/// Permissions the plugin requests at install time. The runtime does
/// not yet enforce permission *gates* (Phase 2 ships denial-by-default
/// — every plugin is fully sandboxed regardless of what it asks for);
/// the field exists so the manifest schema is stable and the UI can
/// show the plugin's stated intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPermission {
    ReadDocument,
    WriteDocument,
    ReadAssets,
    ExportFiles,
    NetworkAccess,
}

/// Parsed plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    /// Relative path inside the plugin directory to the entry-point
    /// file (`.wasm` for WASM, `.js` for JsPanel, exec name for Native).
    pub entry_point: String,
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
}

/// Errors from manifest IO / parsing.
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest json parse: {0}")]
    Json(#[from] serde_json::Error),
    #[error("manifest missing required field: {0}")]
    MissingField(&'static str),
    #[error("manifest entry-point file not found: {0}")]
    EntryPointMissing(String),
}

impl PluginManifest {
    /// Load a manifest from a directory. Looks for `manifest.json` and
    /// verifies the entry-point file exists.
    pub fn load(plugin_dir: &Path) -> Result<Self, ManifestError> {
        let manifest_path = plugin_dir.join("manifest.json");
        let bytes = std::fs::read(&manifest_path)?;
        let manifest: Self = serde_json::from_slice(&bytes)?;
        manifest.validate(plugin_dir)?;
        Ok(manifest)
    }

    fn validate(&self, plugin_dir: &Path) -> Result<(), ManifestError> {
        if self.id.is_empty() {
            return Err(ManifestError::MissingField("id"));
        }
        if self.name.is_empty() {
            return Err(ManifestError::MissingField("name"));
        }
        if self.version.is_empty() {
            return Err(ManifestError::MissingField("version"));
        }
        if self.entry_point.is_empty() {
            return Err(ManifestError::MissingField("entry_point"));
        }
        let entry = plugin_dir.join(&self.entry_point);
        if !entry.exists() {
            return Err(ManifestError::EntryPointMissing(
                entry.display().to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_minimal_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "id": "demo",
                "name": "Demo",
                "version": "0.1.0",
                "type": "wasm",
                "entry_point": "demo.wasm"
            }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("demo.wasm"), b"\0asm").unwrap();
        let m = PluginManifest::load(dir.path()).unwrap();
        assert_eq!(m.id, "demo");
        assert_eq!(m.plugin_type, PluginType::Wasm);
        assert_eq!(m.entry_point, "demo.wasm");
        assert!(m.permissions.is_empty());
    }

    #[test]
    fn parses_permissions_and_metadata() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "id": "demo",
                "name": "Demo",
                "version": "0.1.0",
                "author": "Alice",
                "description": "tests",
                "type": "wasm",
                "entry_point": "demo.wasm",
                "permissions": ["read_document", "write_document"]
            }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("demo.wasm"), b"\0asm").unwrap();
        let m = PluginManifest::load(dir.path()).unwrap();
        assert_eq!(m.author, "Alice");
        assert_eq!(m.description, "tests");
        assert_eq!(
            m.permissions,
            vec![
                PluginPermission::ReadDocument,
                PluginPermission::WriteDocument
            ]
        );
    }

    #[test]
    fn rejects_missing_id() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "id": "",
                "name": "Demo",
                "version": "0.1.0",
                "type": "wasm",
                "entry_point": "demo.wasm"
            }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("demo.wasm"), b"\0asm").unwrap();
        let err = PluginManifest::load(dir.path()).unwrap_err();
        assert!(matches!(err, ManifestError::MissingField("id")));
    }

    #[test]
    fn rejects_missing_entry_point_file() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "id": "demo",
                "name": "Demo",
                "version": "0.1.0",
                "type": "wasm",
                "entry_point": "missing.wasm"
            }"#,
        )
        .unwrap();
        let err = PluginManifest::load(dir.path()).unwrap_err();
        assert!(matches!(err, ManifestError::EntryPointMissing(_)));
    }

    #[test]
    fn rejects_garbage_json() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("manifest.json"), b"not json").unwrap();
        let err = PluginManifest::load(dir.path()).unwrap_err();
        assert!(matches!(err, ManifestError::Json(_)));
    }
}
