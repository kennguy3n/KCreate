//! Plugin marketplace — Phase 10 Block D Task 20.
//!
//! Discovery + lifecycle layer on top of [`crate::registry`].
//!
//! - **List** mirrors the existing template marketplace's surface:
//!   per-plugin id, name, version, author, permissions, trust status,
//!   and the enabled flag.
//! - **Install local** copies a plugin bundle (a directory or zip
//!   containing `manifest.json` + entry-point) into the host's
//!   plugin directory.
//! - **Remove** drops the plugin directory from disk and any
//!   matching enable-state from `enabled.json`.
//!
//! The marketplace owns no state of its own — it spins up a fresh
//! [`PluginRegistry`] per call so this module is safe to use from
//! the bridge without any locks.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zip::ZipArchive;

use crate::manifest::{ManifestError, PluginManifest, PluginPermission};
use crate::registry::{PluginRegistry, RegistryError, SignatureStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginListing {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub permissions: Vec<String>,
    pub trust_status: String,
    pub installed: bool,
}

#[derive(Debug, Error)]
pub enum MarketplaceError {
    #[error("plugin_marketplace: io: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin_marketplace: registry: {0}")]
    Registry(#[from] RegistryError),
    #[error("plugin_marketplace: manifest: {0}")]
    Manifest(#[from] ManifestError),
    #[error("plugin_marketplace: zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("plugin_marketplace: source path does not exist: {0}")]
    NotFound(PathBuf),
    #[error("plugin_marketplace: source is neither a directory nor a .zip/.kcplugin bundle: {0}")]
    UnsupportedSource(PathBuf),
    #[error("plugin_marketplace: plugin {0} is already installed")]
    AlreadyInstalled(String),
    #[error("plugin_marketplace: plugin {0} is not installed")]
    NotInstalled(String),
    #[error("plugin_marketplace: manifest.id {0:?} is not a safe path component")]
    InvalidPluginId(String),
}

/// Validate that `id` is safe to use as a single path component
/// under the plugin root. Rejects empty strings, anything containing
/// path separators (`/` or `\`), parent-directory references
/// (`..`), NUL bytes, or names that don't round-trip through
/// `Path::file_name()`. This is the security gate for plugin
/// install / remove because `manifest.id` comes from an untrusted
/// bundle and is joined onto `plugin_root` to form a destination
/// path. Without this check a hostile bundle could write to (or
/// delete) arbitrary directories outside `~/.kcreate/plugins/`.
fn validate_plugin_id(id: &str) -> Result<(), MarketplaceError> {
    if id.is_empty() {
        return Err(MarketplaceError::InvalidPluginId(id.into()));
    }
    if id.contains('/')
        || id.contains('\\')
        || id.contains('\0')
        || id == "."
        || id == ".."
        || id.split(['/', '\\']).any(|seg| seg == "..")
    {
        return Err(MarketplaceError::InvalidPluginId(id.into()));
    }
    // Final sanity check: the id must round-trip through Path's
    // own component extractor and equal itself. This catches
    // OS-specific oddities (drive letters, UNC prefixes, etc.) that
    // the explicit byte checks above might miss.
    let p = Path::new(id);
    if p.file_name().and_then(|s| s.to_str()) != Some(id) {
        return Err(MarketplaceError::InvalidPluginId(id.into()));
    }
    if p.components().count() != 1 {
        return Err(MarketplaceError::InvalidPluginId(id.into()));
    }
    Ok(())
}

/// Default location for installed plugins — `~/.kcreate/plugins/`.
///
/// Resolves the user's home directory in a cross-platform way:
/// `HOME` (Linux / macOS) → `USERPROFILE` (Windows). Mirrors the
/// fallback chain already used by `kcreate_core::marketplace` and
/// `kcreate_bridge::phase2`, so plugin discovery works on the
/// `windows-2022` CI matrix where `HOME` is not exported.
#[must_use]
pub fn default_plugin_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".kcreate").join("plugins"))
}

#[derive(Debug, Clone)]
pub struct PluginMarketplace {
    plugin_dir: PathBuf,
}

impl Default for PluginMarketplace {
    fn default() -> Self {
        let dir =
            default_plugin_dir().unwrap_or_else(|| std::env::temp_dir().join("kcreate-plugins"));
        Self { plugin_dir: dir }
    }
}

impl PluginMarketplace {
    /// Create a marketplace rooted at `plugin_dir`. The directory is
    /// created on demand by [`Self::list`] / [`Self::install_local`].
    #[must_use]
    pub fn new(plugin_dir: PathBuf) -> Self {
        Self { plugin_dir }
    }

    /// Enumerate installed plugins. Defensively skips any on-disk
    /// manifest whose `id` would not pass [`validate_plugin_id`] — a
    /// hostile id that somehow predates the install-time check
    /// would otherwise be reachable via [`Self::remove`].
    pub fn list(&self) -> Result<Vec<PluginListing>, MarketplaceError> {
        self.ensure_dir()?;
        let mut reg = PluginRegistry::new(self.plugin_dir.clone());
        reg.scan()?;
        let mut out = Vec::new();
        for manifest in reg.list() {
            if validate_plugin_id(&manifest.id).is_err() {
                continue;
            }
            let sig = reg.signature_status_for(&manifest.id).cloned();
            out.push(make_listing(manifest, sig.as_ref(), true));
        }
        Ok(out)
    }

    /// Install a plugin from a local source: either a directory
    /// containing `manifest.json` or a `.zip`/`.kcplugin` bundle.
    pub fn install_local(&self, source: &Path) -> Result<PluginListing, MarketplaceError> {
        if !source.exists() {
            return Err(MarketplaceError::NotFound(source.into()));
        }
        self.ensure_dir()?;
        let staged_dir = if source.is_dir() {
            stage_from_dir(source, &self.plugin_dir)?
        } else {
            match source.extension().and_then(|e| e.to_str()) {
                Some("zip" | "kcplugin") => stage_from_zip(source, &self.plugin_dir)?,
                _ => return Err(MarketplaceError::UnsupportedSource(source.into())),
            }
        };
        let (manifest, _raw) = PluginManifest::load_with_raw(&staged_dir)?;
        // Reject duplicates by id — refuse-and-cleanup so the on-disk
        // state stays consistent even on the error path.
        let mut reg = PluginRegistry::new(self.plugin_dir.clone());
        reg.scan()?;
        let already = reg.list().iter().any(|m| {
            m.id == manifest.id
                && reg_dir_for(&self.plugin_dir, m).is_ok_and(|d| Path::new(&staged_dir) != d)
        });
        if already {
            let _ = fs::remove_dir_all(&staged_dir);
            return Err(MarketplaceError::AlreadyInstalled(manifest.id));
        }
        // Re-scan to pick up the new plugin and surface signature
        // status.
        let mut reg2 = PluginRegistry::new(self.plugin_dir.clone());
        reg2.scan()?;
        let sig = reg2.signature_status_for(&manifest.id).cloned();
        Ok(make_listing(&manifest, sig.as_ref(), true))
    }

    /// Remove an installed plugin by id. Returns `true` if a plugin
    /// was actually removed; `false` if no plugin with that id was
    /// installed. Rejects ids that aren't safe single path
    /// components — even if a malicious manifest somehow got onto
    /// disk, an attacker cannot turn `remove("../../evil")` into an
    /// arbitrary directory deletion.
    pub fn remove(&self, id: &str) -> Result<bool, MarketplaceError> {
        validate_plugin_id(id)?;
        self.ensure_dir()?;
        let mut reg = PluginRegistry::new(self.plugin_dir.clone());
        reg.scan()?;
        let manifest = match reg.list().iter().find(|m| m.id == id).copied() {
            Some(m) => m.clone(),
            None => return Ok(false),
        };
        let dir = reg_dir_for(&self.plugin_dir, &manifest)?;
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(true)
    }

    fn ensure_dir(&self) -> Result<(), MarketplaceError> {
        fs::create_dir_all(&self.plugin_dir)?;
        Ok(())
    }
}

fn make_listing(
    manifest: &PluginManifest,
    sig: Option<&SignatureStatus>,
    installed: bool,
) -> PluginListing {
    PluginListing {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        author: manifest.author.clone(),
        description: manifest.description.clone(),
        permissions: manifest
            .permissions
            .iter()
            .copied()
            .map(permission_str)
            .collect(),
        trust_status: trust_status_str(sig),
        installed,
    }
}

fn permission_str(p: PluginPermission) -> String {
    format!("{p:?}")
}

fn trust_status_str(sig: Option<&SignatureStatus>) -> String {
    match sig {
        Some(SignatureStatus::Verified { key_id }) => format!("verified:{key_id}"),
        Some(SignatureStatus::Invalid { key_id, reason }) => {
            format!("invalid:{key_id}:{reason}")
        }
        Some(SignatureStatus::Unsigned) | None => "unsigned".into(),
    }
}

fn reg_dir_for(root: &Path, manifest: &PluginManifest) -> Result<PathBuf, MarketplaceError> {
    validate_plugin_id(&manifest.id)?;
    Ok(root.join(&manifest.id))
}

fn stage_from_dir(source: &Path, plugin_root: &Path) -> Result<PathBuf, MarketplaceError> {
    let (manifest, _raw) = PluginManifest::load_with_raw(source)?;
    validate_plugin_id(&manifest.id)?;
    let dest = plugin_root.join(&manifest.id);
    if dest.exists() {
        return Err(MarketplaceError::AlreadyInstalled(manifest.id));
    }
    copy_dir_recursive(source, &dest)?;
    Ok(dest)
}

/// Remove orphaned `.staging-*` directories under `plugin_root`.
/// A previous install that crashed between extract and rename can
/// leave behind a `.staging-{pid}` dir from a now-dead process.
/// Called at the top of every install so disk usage stays bounded
/// even across repeated crashes — leftovers from the *current*
/// process are also cleared, which keeps the per-pid staging path
/// safe to recreate immediately after.
fn sweep_stale_staging(plugin_root: &Path) {
    let Ok(entries) = fs::read_dir(plugin_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(".staging-") {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
}

fn stage_from_zip(source: &Path, plugin_root: &Path) -> Result<PathBuf, MarketplaceError> {
    let file = fs::File::open(source)?;
    let mut archive = ZipArchive::new(file)?;
    // Sweep orphan staging dirs from prior crashed installs before
    // claiming the per-pid staging path for this run.
    sweep_stale_staging(plugin_root);
    let staging = plugin_root.join(format!(".staging-{}", std::process::id()));
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    fs::create_dir_all(&staging)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let out_path = staging.join(&name);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut writer = fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut writer)?;
        }
    }
    let (manifest, _raw) = PluginManifest::load_with_raw(&staging)?;
    if let Err(e) = validate_plugin_id(&manifest.id) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    let dest = plugin_root.join(&manifest.id);
    if dest.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(MarketplaceError::AlreadyInstalled(manifest.id));
    }
    fs::rename(&staging, &dest).or_else(|_| {
        // rename failed (different filesystem) — fall back to copy.
        copy_dir_recursive(&staging, &dest)?;
        fs::remove_dir_all(&staging)
    })?;
    Ok(dest)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_min_manifest(dir: &Path, id: &str) -> std::io::Result<()> {
        let manifest = format!(
            r#"{{
              "id": "{id}",
              "name": "{id}",
              "version": "0.0.1",
              "author": "tester",
              "description": "",
              "type": "wasm",
              "entry_point": "plugin.wasm",
              "permissions": []
            }}"#
        );
        fs::write(dir.join("manifest.json"), manifest)?;
        fs::write(dir.join("plugin.wasm"), b"\0asm\x01\0\0\0")?;
        Ok(())
    }

    #[test]
    fn validate_plugin_id_rejects_path_traversal() {
        // Every shape an attacker might try to break out of the
        // plugin root with must be rejected.
        let bad = [
            "",
            ".",
            "..",
            "../evil",
            "..\\evil",
            "../../etc/passwd",
            "a/b",
            "a\\b",
            "/abs",
            "\\abs",
            "foo\0bar",
            "foo/../bar",
        ];
        for id in bad {
            assert!(
                validate_plugin_id(id).is_err(),
                "expected {id:?} to be rejected"
            );
        }
        // The legitimate id shapes used by real plugins must still
        // pass.
        for id in ["a", "plugin", "com.example.plugin", "my-plugin_v2"] {
            assert!(validate_plugin_id(id).is_ok(), "expected {id:?} to pass");
        }
    }

    #[test]
    fn install_rejects_traversal_id_in_manifest() {
        // A malicious plugin bundle with `id: "../../evil"` MUST be
        // rejected before any directory is created outside the plugin
        // root. We assert both that the install fails with
        // `InvalidPluginId` AND that no escape directory was created.
        let staging = TempDir::new().unwrap();
        let plugins = TempDir::new().unwrap();
        let src = staging.path().join("hostile");
        fs::create_dir_all(&src).unwrap();
        write_min_manifest(&src, "../../escape").unwrap();
        let mp = PluginMarketplace::new(plugins.path().join("plugins"));
        let err = mp.install_local(&src).unwrap_err();
        assert!(
            matches!(err, MarketplaceError::InvalidPluginId(_)),
            "expected InvalidPluginId, got {err:?}"
        );
        // The plugin root itself was created by ensure_dir(); but no
        // sibling "escape" directory should exist anywhere near it.
        assert!(!plugins.path().join("escape").exists());
        assert!(!plugins.path().parent().unwrap().join("escape").exists());
    }

    #[test]
    fn remove_rejects_traversal_id() {
        // Even if a hostile id somehow ends up in a `remove()` call,
        // we must refuse it rather than turning it into
        // `fs::remove_dir_all("../../something")`.
        let plugins = TempDir::new().unwrap();
        let mp = PluginMarketplace::new(plugins.path().to_path_buf());
        let err = mp.remove("../../etc").unwrap_err();
        assert!(matches!(err, MarketplaceError::InvalidPluginId(_)));
    }

    #[test]
    fn list_empty_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let mp = PluginMarketplace::new(dir.path().join("plugins"));
        assert!(mp.list().unwrap().is_empty());
    }

    #[test]
    fn install_from_directory_round_trips_through_list() {
        let staging = TempDir::new().unwrap();
        let plugins = TempDir::new().unwrap();
        let src = staging.path().join("hello");
        fs::create_dir_all(&src).unwrap();
        write_min_manifest(&src, "test.hello").unwrap();
        let mp = PluginMarketplace::new(plugins.path().to_path_buf());
        let listing = mp.install_local(&src).unwrap();
        assert_eq!(listing.id, "test.hello");
        let listed = mp.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "test.hello");
    }

    #[test]
    fn install_twice_errors() {
        let staging = TempDir::new().unwrap();
        let plugins = TempDir::new().unwrap();
        let src = staging.path().join("dupe");
        fs::create_dir_all(&src).unwrap();
        write_min_manifest(&src, "test.dupe").unwrap();
        let mp = PluginMarketplace::new(plugins.path().to_path_buf());
        let _ = mp.install_local(&src).unwrap();
        let err = mp.install_local(&src).unwrap_err();
        assert!(matches!(err, MarketplaceError::AlreadyInstalled(_)));
    }

    #[test]
    fn remove_returns_false_when_nothing_to_remove() {
        let plugins = TempDir::new().unwrap();
        let mp = PluginMarketplace::new(plugins.path().to_path_buf());
        let removed = mp.remove("nope").unwrap();
        assert!(!removed);
    }

    #[test]
    fn remove_drops_disk_dir() {
        let staging = TempDir::new().unwrap();
        let plugins = TempDir::new().unwrap();
        let src = staging.path().join("rm");
        fs::create_dir_all(&src).unwrap();
        write_min_manifest(&src, "test.rm").unwrap();
        let mp = PluginMarketplace::new(plugins.path().to_path_buf());
        mp.install_local(&src).unwrap();
        assert!(mp.remove("test.rm").unwrap());
        assert!(mp.list().unwrap().is_empty());
    }

    #[test]
    fn sweep_stale_staging_removes_orphan_dirs() {
        // Simulate a crashed install: a `.staging-<pid>` dir from
        // a now-dead process is left behind in the plugin root.
        // The sweep must reclaim it (and leave the real plugin
        // directories untouched).
        let plugins = TempDir::new().unwrap();
        let root = plugins.path();
        fs::create_dir_all(root).unwrap();
        // Two orphan staging dirs (different pids) + one orphan
        // staging file + one legitimate plugin dir.
        let orphan_a = root.join(".staging-99991");
        let orphan_b = root.join(".staging-99992");
        let orphan_file = root.join(".staging-stray");
        let real_plugin = root.join("real.plugin");
        fs::create_dir_all(&orphan_a).unwrap();
        fs::write(orphan_a.join("junk"), b"x").unwrap();
        fs::create_dir_all(&orphan_b).unwrap();
        fs::write(&orphan_file, b"y").unwrap();
        fs::create_dir_all(&real_plugin).unwrap();
        write_min_manifest(&real_plugin, "real.plugin").unwrap();

        sweep_stale_staging(root);

        assert!(!orphan_a.exists(), ".staging-99991 should be swept");
        assert!(!orphan_b.exists(), ".staging-99992 should be swept");
        assert!(!orphan_file.exists(), ".staging-stray should be swept");
        assert!(
            real_plugin.exists(),
            "real plugin dir must not be touched by the sweep"
        );
    }

    #[test]
    fn sweep_stale_staging_is_idempotent_and_safe_on_missing_root() {
        // Sweeping a directory that doesn't exist (e.g. before the
        // first ever install) must not panic or error.
        let plugins = TempDir::new().unwrap();
        let missing = plugins.path().join("does-not-exist");
        sweep_stale_staging(&missing); // no panic
                                       // And sweeping an empty root is a no-op.
        let empty = plugins.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        sweep_stale_staging(&empty);
        assert!(empty.exists());
    }
}
