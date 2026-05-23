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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// JS-panel-specific config; required when `plugin_type` is
    /// `JsPanel` and ignored otherwise. Stored on the manifest itself
    /// (rather than a sidecar file) so the registry only has to read
    /// one JSON file per plugin and so panel authors don't have to
    /// keep two files in sync.
    #[serde(default, rename = "js_panel", skip_serializing_if = "Option::is_none")]
    pub js_panel: Option<crate::js_panel::JsPanelConfig>,
}

/// On-disk shape of `manifest.json.sig`. The sidecar carries the
/// `key_id` (which the trust store maps to a public key) and the
/// raw Ed25519 signature over the *bytes of `manifest.json`*. The
/// signed message is the file bytes verbatim — no canonical-JSON
/// re-serialisation, no field omission games. This makes the
/// "what got signed" agreement bit-exact across signing tools and
/// JSON libraries. See [`crate::trust`] for the verification entry
/// point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSignature {
    pub key_id: String,
    /// Base64-URL (no padding) of the 64-byte Ed25519 signature.
    pub signature_b64: String,
}

impl PluginSignature {
    /// Load `manifest.json.sig` from a plugin directory. Returns
    /// `Ok(None)` if the sidecar file does not exist (unsigned
    /// plugin); the caller's policy (e.g. "native plugins must be
    /// signed") decides what to do with that.
    pub fn load_optional(plugin_dir: &Path) -> Result<Option<Self>, ManifestError> {
        let sig_path = plugin_dir.join("manifest.json.sig");
        if !sig_path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&sig_path)?;
        let sig: Self = serde_json::from_slice(&bytes)?;
        if sig.key_id.is_empty() {
            return Err(ManifestError::MissingField("signature.key_id"));
        }
        if sig.signature_b64.is_empty() {
            return Err(ManifestError::MissingField("signature.signature_b64"));
        }
        Ok(Some(sig))
    }
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
    /// A manifest path (e.g. `entry_point`, `js_panel.entry_html`)
    /// resolved *outside* the plugin directory after symlink and `..`
    /// canonicalisation. This is treated as a hard load failure so a
    /// malicious manifest can never trick the Electron host into
    /// `file://`-loading `/etc/passwd` or any other host file by
    /// crafting a path like `../../../../../etc/passwd`.
    #[error("manifest path {referenced} escapes plugin directory {plugin_dir}")]
    PathEscape {
        referenced: String,
        plugin_dir: String,
    },
}

impl PluginManifest {
    /// Load a manifest from a directory. Looks for `manifest.json` and
    /// verifies the entry-point file exists.
    pub fn load(plugin_dir: &Path) -> Result<Self, ManifestError> {
        Self::load_with_raw(plugin_dir).map(|(m, _)| m)
    }

    /// Same as [`Self::load`] but also returns the raw bytes of
    /// `manifest.json` as they were read off disk. The registry uses
    /// this to verify the optional `manifest.json.sig` sidecar
    /// without re-reading the file — and, crucially, to verify
    /// against the *exact bytes* the signer signed, with no
    /// canonical-JSON re-serialisation step between disk and
    /// verification.
    pub fn load_with_raw(plugin_dir: &Path) -> Result<(Self, Vec<u8>), ManifestError> {
        let manifest_path = plugin_dir.join("manifest.json");
        let bytes = std::fs::read(&manifest_path)?;
        let manifest: Self = serde_json::from_slice(&bytes)?;
        manifest.validate(plugin_dir)?;
        Ok((manifest, bytes))
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
        // `entry_point` must (a) exist on disk and (b) resolve to a
        // path *inside* `plugin_dir` after symlink + `..`
        // canonicalisation. A manifest carrying
        // `"entry_point": "../../../../../etc/passwd"` would otherwise
        // be readable by the WASM loader (or, worse, file://-loadable
        // by Electron in the js_panel case below).
        let entry = plugin_dir.join(&self.entry_point);
        if !entry.exists() {
            return Err(ManifestError::EntryPointMissing(
                entry.display().to_string(),
            ));
        }
        ensure_path_within(plugin_dir, &entry)?;
        if self.plugin_type == PluginType::JsPanel {
            let Some(cfg) = &self.js_panel else {
                return Err(ManifestError::MissingField("js_panel"));
            };
            if cfg.entry_html.is_empty() {
                return Err(ManifestError::MissingField("js_panel.entry_html"));
            }
            // The js_panel.entry_html file must also exist on disk so
            // the Electron host can `file://`-load it. We check that
            // here rather than letting the Electron main process
            // discover a 404 at panel-open time. We *also* check
            // containment so the host can never be tricked into
            // `file://`-loading anything outside the plugin sandbox.
            let html = plugin_dir.join(&cfg.entry_html);
            if !html.exists() {
                return Err(ManifestError::EntryPointMissing(
                    html.display().to_string(),
                ));
            }
            ensure_path_within(plugin_dir, &html)?;
        }
        Ok(())
    }
}

/// Returns `Ok(())` iff `candidate` resolves to a path inside
/// `plugin_dir` after both sides are run through
/// [`std::fs::canonicalize`]. Canonicalisation walks every symlink
/// and collapses every `..` so a manifest cannot escape via either
/// trick.
///
/// Both paths are required to be canonicalisable — i.e. they must
/// already exist on disk. The callers above check existence first,
/// so this is always true in practice; if either canonicalisation
/// fails we propagate the underlying `io::Error` rather than
/// invent a `PathEscape`, because the user can't act on a phantom
/// containment violation.
fn ensure_path_within(plugin_dir: &Path, candidate: &Path) -> Result<(), ManifestError> {
    let root = std::fs::canonicalize(plugin_dir)?;
    let resolved = std::fs::canonicalize(candidate)?;
    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(ManifestError::PathEscape {
            referenced: resolved.display().to_string(),
            plugin_dir: root.display().to_string(),
        })
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

    #[test]
    fn parses_js_panel_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "id": "panel",
                "name": "Panel",
                "version": "0.1.0",
                "type": "js_panel",
                "entry_point": "panel.html",
                "permissions": ["read_document"],
                "js_panel": {
                    "entry_html": "panel.html",
                    "panel_title": "Panel",
                    "panel_position": "right_sidebar",
                    "width": 320,
                    "height": 480,
                    "permissions": ["read_document"]
                }
            }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("panel.html"), b"<!doctype html><html></html>").unwrap();
        let m = PluginManifest::load(dir.path()).unwrap();
        assert_eq!(m.plugin_type, PluginType::JsPanel);
        let cfg = m.js_panel.as_ref().expect("js_panel must be present");
        assert_eq!(cfg.entry_html, "panel.html");
        assert_eq!(cfg.width, 320);
        assert!(cfg.has(PluginPermission::ReadDocument));
    }

    #[test]
    fn rejects_js_panel_without_config() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "id": "panel",
                "name": "Panel",
                "version": "0.1.0",
                "type": "js_panel",
                "entry_point": "panel.html"
            }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("panel.html"), b"<!doctype html><html></html>").unwrap();
        let err = PluginManifest::load(dir.path()).unwrap_err();
        assert!(matches!(err, ManifestError::MissingField("js_panel")));
    }

    #[test]
    fn rejects_js_panel_with_missing_html() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "id": "panel",
                "name": "Panel",
                "version": "0.1.0",
                "type": "js_panel",
                "entry_point": "panel.html",
                "js_panel": {
                    "entry_html": "ghost.html",
                    "panel_title": "Panel",
                    "panel_position": "right_sidebar",
                    "width": 320,
                    "height": 480
                }
            }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("panel.html"), b"<!doctype html><html></html>").unwrap();
        let err = PluginManifest::load(dir.path()).unwrap_err();
        assert!(matches!(err, ManifestError::EntryPointMissing(_)));
    }

    /// A manifest whose `entry_point` escapes the plugin directory via
    /// `..` traversal must be rejected with [`ManifestError::PathEscape`],
    /// not silently followed. We materialise a real file outside the
    /// plugin dir (`escape.wasm` in the temp root) and point
    /// `entry_point` at it through `..` so `Path::exists()` returns
    /// `true` and only the containment check can stop the load.
    #[test]
    fn rejects_entry_point_path_traversal_escape() {
        let outer = tempdir().unwrap();
        let plugin_dir = outer.path().join("plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        // The "escape target" — a file that exists *outside* the
        // plugin directory.
        std::fs::write(outer.path().join("escape.wasm"), b"\0asm").unwrap();
        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{
                "id": "evil",
                "name": "Evil",
                "version": "0.1.0",
                "type": "wasm",
                "entry_point": "../escape.wasm"
            }"#,
        )
        .unwrap();
        let err = PluginManifest::load(&plugin_dir).unwrap_err();
        assert!(
            matches!(err, ManifestError::PathEscape { .. }),
            "expected PathEscape, got {err:?}",
        );
    }

    /// Same containment guarantee for `js_panel.entry_html`, which is
    /// the higher-risk case — the Electron host `file://`-loads it
    /// into a sandboxed WebContentsView, so a successful traversal
    /// would let a malicious manifest display arbitrary host files
    /// inside the editor.
    #[test]
    fn rejects_js_panel_entry_html_path_traversal_escape() {
        let outer = tempdir().unwrap();
        let plugin_dir = outer.path().join("plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(
            outer.path().join("secrets.html"),
            b"<!doctype html><html><body>secret</body></html>",
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{
                "id": "evil-panel",
                "name": "Evil Panel",
                "version": "0.1.0",
                "type": "js_panel",
                "entry_point": "panel.html",
                "js_panel": {
                    "entry_html": "../secrets.html",
                    "panel_title": "Evil",
                    "panel_position": "right_sidebar",
                    "width": 320,
                    "height": 480
                }
            }"#,
        )
        .unwrap();
        // entry_point still has to exist inside the plugin dir so we
        // reach the js_panel check.
        std::fs::write(
            plugin_dir.join("panel.html"),
            b"<!doctype html><html></html>",
        )
        .unwrap();
        let err = PluginManifest::load(&plugin_dir).unwrap_err();
        assert!(
            matches!(err, ManifestError::PathEscape { .. }),
            "expected PathEscape, got {err:?}",
        );
    }
}
