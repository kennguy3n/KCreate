//! Local template marketplace — Phase 3 (local-only).
//!
//! The marketplace scans a configurable directory (`~/.kcreate/templates/`)
//! for `.ktemplate/` folders, each containing a `manifest.json` that
//! describes the template (name, category, tags, thumbnail, pages).
//!
//! Phase 3 is strictly local: templates are discovered on disk, installed
//! by copying the folder, and removed by deleting it. A future phase may
//! add a remote `TemplateSource::Marketplace` for a hosted catalogue.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::project::TemplateCategory;

/// Where a template was sourced from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum TemplateSource {
    /// Installed on the local filesystem (Phase 3).
    Local { path: PathBuf },
}

/// Metadata for an installed template, read from `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateManifest {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub category: TemplateCategory,
    pub tags: Vec<String>,
    /// Relative path to a thumbnail image inside the template folder.
    pub thumbnail: Option<String>,
    /// Number of pages the template generates.
    pub page_count: u32,
    pub author: Option<String>,
    pub version: String,
    #[serde(default)]
    pub source: Option<TemplateSource>,
}

/// Specification for importing an external design as a new library
/// template (the "Remix from file" flow).
///
/// This type is deliberately **format-agnostic**: extracting an
/// external `.kstudio` / `.ktemplate` / template `content.json` into
/// the wire-format `content_json` string is the caller's job (it lives
/// in the bridge, which can link `kcreate_storage` to read a SQLite
/// project — `kcreate_core` cannot, to avoid a dependency cycle).
/// [`LocalMarketplace::import_design`] owns only persistence +
/// registration, so the import pipeline keeps a single, testable
/// seam for "turn an arbitrary design into a library entry".
#[derive(Debug, Clone)]
pub struct ImportSpec {
    /// Display name for the new template.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Library category the imported design is filed under.
    pub category: TemplateCategory,
    /// Search tags.
    pub tags: Vec<String>,
    /// Number of pages/artboards the design contains (clamped to `>= 1`).
    pub page_count: u32,
    /// Wire-format template content (`{ "width", "height", "items": [...] }`)
    /// already serialised to JSON. Persisted verbatim as `content.json`
    /// so the imported entry drives the gallery thumbnail and the
    /// applied canvas through the exact same path as a bundled template.
    pub content_json: String,
}

/// Errors from marketplace operations.
#[derive(Debug, Error)]
pub enum MarketplaceError {
    #[error("template directory does not exist: {0}")]
    DirectoryNotFound(PathBuf),
    #[error("manifest parse error in {path}: {reason}")]
    ManifestParse { path: PathBuf, reason: String },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("template {0} not found")]
    TemplateNotFound(Uuid),
    #[error("template {0} already installed")]
    AlreadyInstalled(Uuid),
    #[error("serialize error: {0}")]
    Serialize(String),
    #[error("imported design content is not valid template JSON: {0}")]
    InvalidContent(String),
}

/// The local template marketplace — scans, lists, installs, and
/// removes `.ktemplate/` folders from a configurable root directory.
#[derive(Debug, Clone)]
pub struct LocalMarketplace {
    root: PathBuf,
    templates: HashMap<Uuid, TemplateManifest>,
}

impl LocalMarketplace {
    /// Create a new marketplace rooted at `dir`. The directory is
    /// created on first `scan()` if it doesn't exist.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            root: dir.into(),
            templates: HashMap::new(),
        }
    }

    /// The default template directory: `~/.kcreate/templates/`.
    #[must_use]
    pub fn default_dir() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".kcreate").join("templates")
    }

    /// Root directory this marketplace scans.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Scan the root directory for `.ktemplate/` folders containing
    /// a `manifest.json`. Returns the number of templates discovered.
    /// Invalid manifests are silently skipped — a single corrupt
    /// template should not prevent the rest from loading.
    pub fn scan(&mut self) -> Result<usize, MarketplaceError> {
        self.templates.clear();
        if !self.root.exists() {
            std::fs::create_dir_all(&self.root)?;
        }
        let entries = std::fs::read_dir(&self.root)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            // Sweep leftover staging dirs from an import that was
            // interrupted (e.g. process killed) between staging and the
            // atomic rename. They never end in `.ktemplate`, so they are
            // invisible to the loader anyway; removing them on scan (the
            // natural startup GC point, serialised via `&mut self`) keeps
            // crash litter from accumulating.
            if dir_name.ends_with(".ktemplate.partial") {
                let _ = std::fs::remove_dir_all(&path);
                continue;
            }
            if !dir_name.ends_with(".ktemplate") {
                continue;
            }
            let manifest_path = path.join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            match read_manifest(&manifest_path) {
                Ok(mut manifest) => {
                    manifest.source = Some(TemplateSource::Local { path: path.clone() });
                    self.templates.insert(manifest.id, manifest);
                }
                Err(_) => {
                    log::warn!(
                        "skipping invalid template manifest: {}",
                        manifest_path.display()
                    );
                }
            }
        }
        Ok(self.templates.len())
    }

    /// All discovered templates, sorted by name.
    #[must_use]
    pub fn list(&self) -> Vec<&TemplateManifest> {
        let mut v: Vec<_> = self.templates.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Filter templates by category.
    #[must_use]
    pub fn filter_by_category(&self, category: TemplateCategory) -> Vec<&TemplateManifest> {
        let mut v: Vec<_> = self
            .templates
            .values()
            .filter(|t| t.category == category)
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Search templates by name, tag, or description (case-insensitive
    /// substring), ranked by *where* the query matched: a name hit
    /// outranks a tag hit, which outranks a description-only hit. Within
    /// the same rank, results are ordered alphabetically by name so the
    /// ordering stays stable across runs (the `templates` map iterates
    /// in arbitrary order).
    ///
    /// The matched *set* is identical to a plain
    /// name-OR-tag-OR-description filter — ranking only reorders, never
    /// drops or adds — so callers that count matches stay correct.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&TemplateManifest> {
        let q = query.to_lowercase();
        let mut ranked: Vec<(u8, &TemplateManifest)> = self
            .templates
            .values()
            .filter_map(|t| search_rank(t, &q).map(|rank| (rank, t)))
            .collect();
        ranked.sort_by(|(ra, a), (rb, b)| ra.cmp(rb).then_with(|| a.name.cmp(&b.name)));
        ranked.into_iter().map(|(_, t)| t).collect()
    }

    /// Get a template by id.
    #[must_use]
    pub fn get(&self, id: Uuid) -> Option<&TemplateManifest> {
        self.templates.get(&id)
    }

    /// Install a local template from `source_dir` into the
    /// marketplace root. The source must be a `.ktemplate/` folder
    /// with a valid `manifest.json`. Copies the entire directory.
    pub fn install_local(
        &mut self,
        source_dir: &Path,
    ) -> Result<TemplateManifest, MarketplaceError> {
        let manifest_path = source_dir.join("manifest.json");
        let manifest = read_manifest(&manifest_path)?;
        if self.templates.contains_key(&manifest.id) {
            return Err(MarketplaceError::AlreadyInstalled(manifest.id));
        }
        let dir_name = source_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("template.ktemplate");
        let dest = self.root.join(dir_name);
        if !self.root.exists() {
            std::fs::create_dir_all(&self.root)?;
        }
        copy_dir_recursive(source_dir, &dest)?;
        let mut installed = manifest;
        installed.source = Some(TemplateSource::Local { path: dest });
        self.templates.insert(installed.id, installed.clone());
        Ok(installed)
    }

    /// Import an external design as a brand-new library template.
    ///
    /// The caller supplies a wire-format `content.json` (already
    /// extracted from whatever source format — see [`ImportSpec`]) plus
    /// the metadata to file it under. A fresh v4 UUID is minted (so an
    /// import never collides with a bundled id or a previous import of
    /// the same source) and the design is written to a collision-free
    /// `<slug>-<id8>.ktemplate/` folder under the marketplace root,
    /// then registered in memory so it shows up in [`Self::list`] /
    /// [`Self::get`] without a rescan.
    ///
    /// The `content_json` is validated against the canonical
    /// [`crate::template_library::TemplateContent`] shape **before**
    /// anything is written, so a malformed import fails fast and never
    /// leaves a half-written `.ktemplate/` on disk that a later
    /// [`Self::scan`] would surface as a broken entry.
    ///
    /// The two files are written into a temporary sibling directory and
    /// then **atomically renamed** into the final `.ktemplate/` location
    /// (same filesystem, so the rename is atomic). The staging name
    /// deliberately does not end in `.ktemplate`, so a concurrent
    /// [`Self::scan`] — which only considers `.ktemplate/` dirs that
    /// carry a `manifest.json` — can never observe a directory that has
    /// the manifest written but not yet the `content.json`. On any write
    /// failure the staging dir is removed, so a partial entry is never
    /// left behind.
    ///
    /// No `thumbnail.png` is written: the lazy, content-hash-keyed
    /// thumbnail cache (in the bridge) renders it on first view from
    /// the same `content.json`, guaranteeing the preview matches the
    /// applied canvas.
    pub fn import_design(
        &mut self,
        spec: ImportSpec,
    ) -> Result<TemplateManifest, MarketplaceError> {
        // Validate the content up front using the canonical template
        // type. This rejects garbage before we touch the filesystem and
        // guarantees the written entry round-trips through seeding,
        // listing, instantiation, and thumbnail rendering.
        serde_json::from_str::<crate::template_library::TemplateContent>(&spec.content_json)
            .map_err(|e| MarketplaceError::InvalidContent(e.to_string()))?;

        if !self.root.exists() {
            std::fs::create_dir_all(&self.root)?;
        }
        let id = Uuid::new_v4();
        let slug = slugify(&spec.name);
        // `<slug>-<id8>` keeps folder names readable while the 8-hex
        // suffix makes collisions (same name imported twice) impossible.
        let short = &id.simple().to_string()[..8];
        let dir_name = format!("{slug}-{short}.ktemplate");
        let dest = self.root.join(&dir_name);
        if dest.exists() {
            // The 8-hex suffix makes this astronomically unlikely, but
            // never silently overwrite an existing template directory.
            return Err(MarketplaceError::AlreadyInstalled(id));
        }

        let manifest = TemplateManifest {
            id,
            name: spec.name,
            description: spec.description,
            category: spec.category,
            tags: spec.tags,
            thumbnail: Some("thumbnail.png".to_string()),
            page_count: spec.page_count.max(1),
            author: Some("Imported".to_string()),
            version: "1.0.0".to_string(),
            // On-disk manifests never persist `source`; `scan()` stamps
            // it from the discovered path, matching every other entry.
            source: None,
        };
        let manifest_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| MarketplaceError::Serialize(e.to_string()))?;

        // Stage both files in a sibling temp dir whose name does NOT end
        // in `.ktemplate` (so a concurrent `scan()` ignores it), then
        // atomically publish with a single rename. Clean up the staging
        // dir on any failure so we never leave a partial entry on disk.
        let staging = self.root.join(format!("{dir_name}.partial"));
        if staging.exists() {
            std::fs::remove_dir_all(&staging)?;
        }
        if let Err(e) = stage_and_publish(&staging, &dest, &manifest_json, &spec.content_json) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e.into());
        }

        // Register in memory with the same `Local { path }` stamp a
        // `scan()` would apply, so the freshly imported template is
        // immediately visible without a rescan.
        let mut registered = manifest;
        registered.source = Some(TemplateSource::Local { path: dest });
        self.templates.insert(id, registered.clone());
        Ok(registered)
    }

    /// Seed the bundled, ready-made template catalog into this
    /// marketplace's root directory if it has no `.ktemplate/` folders
    /// yet (copy-if-empty). Returns the number of templates written
    /// (`0` when already populated). Idempotent; honours whatever root
    /// the marketplace was constructed with (e.g. a `KCREATE_TEMPLATE_DIR`
    /// override applied by the bridge). Does not re-scan — callers that
    /// need the freshly-seeded set in memory should call [`Self::scan`]
    /// afterwards.
    pub fn seed_bundled(&self) -> Result<usize, MarketplaceError> {
        crate::template_library::seed_bundled_templates(&self.root)
    }

    /// Remove a template by id. Deletes the `.ktemplate/` folder.
    pub fn remove(&mut self, id: Uuid) -> Result<(), MarketplaceError> {
        let manifest = self
            .templates
            .remove(&id)
            .ok_or(MarketplaceError::TemplateNotFound(id))?;
        if let Some(TemplateSource::Local { path }) = &manifest.source {
            if path.exists() {
                std::fs::remove_dir_all(path)?;
            }
        }
        Ok(())
    }
}

/// Write an imported template's `manifest.json` + `content.json` into
/// `staging`, then atomically rename `staging` onto `dest`. Factored out
/// of [`LocalMarketplace::import_design`] so the whole "write then
/// publish" sequence shares one `?`-propagating error path, letting the
/// caller remove `staging` on any failure (a partial entry is never left
/// behind). After a successful rename `staging` no longer exists.
fn stage_and_publish(
    staging: &Path,
    dest: &Path,
    manifest_json: &str,
    content_json: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(staging)?;
    std::fs::write(staging.join("manifest.json"), manifest_json)?;
    std::fs::write(staging.join("content.json"), content_json)?;
    std::fs::rename(staging, dest)?;
    Ok(())
}

/// Rank a template against an already-lowercased query, or `None` if it
/// doesn't match at all. Lower rank = better: name (0) > tag (1) >
/// description (2). The predicate (match in *any* of the three) is
/// identical to the old name-OR-tag-OR-description filter, so the
/// matched set is unchanged — only the order differs.
fn search_rank(t: &TemplateManifest, q: &str) -> Option<u8> {
    if t.name.to_lowercase().contains(q) {
        Some(0)
    } else if t.tags.iter().any(|tag| tag.to_lowercase().contains(q)) {
        Some(1)
    } else if t.description.to_lowercase().contains(q) {
        Some(2)
    } else {
        None
    }
}

/// Turn an arbitrary display name into a filesystem-safe slug:
/// lowercase ASCII alphanumerics, runs of anything else collapsed to a
/// single `-`, no leading/trailing `-`. Falls back to a fixed stem when
/// the name has no usable characters (e.g. all punctuation / non-ASCII)
/// so the import always produces a valid directory name.
fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "imported-design".to_string()
    } else {
        trimmed.to_string()
    }
}

fn read_manifest(path: &Path) -> Result<TemplateManifest, MarketplaceError> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str::<TemplateManifest>(&raw).map_err(|e| MarketplaceError::ManifestParse {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &dest_path)?;
        } else {
            std::fs::copy(&entry_path, &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest(id: Uuid) -> TemplateManifest {
        TemplateManifest {
            id,
            name: "Test Template".into(),
            description: "A test template".into(),
            category: TemplateCategory::PitchDeck,
            tags: vec!["test".into(), "pitch".into()],
            thumbnail: Some("thumb.png".into()),
            page_count: 5,
            author: Some("KCreate".into()),
            version: "1.0.0".into(),
            source: None,
        }
    }

    fn write_ktemplate(dir: &Path, manifest: &TemplateManifest) {
        std::fs::create_dir_all(dir).unwrap();
        let json = serde_json::to_string_pretty(manifest).unwrap();
        std::fs::write(dir.join("manifest.json"), json).unwrap();
    }

    #[test]
    fn scan_discovers_templates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        write_ktemplate(
            &root.join("deck.ktemplate"),
            &TemplateManifest {
                name: "Beta Deck".into(),
                ..sample_manifest(id1)
            },
        );
        write_ktemplate(
            &root.join("proposal.ktemplate"),
            &TemplateManifest {
                name: "Alpha Proposal".into(),
                category: TemplateCategory::Proposal,
                ..sample_manifest(id2)
            },
        );
        // Non-ktemplate dir should be ignored.
        std::fs::create_dir_all(root.join("random_dir")).unwrap();

        let mut mp = LocalMarketplace::new(&root);
        let count = mp.scan().unwrap();
        assert_eq!(count, 2);
        let list = mp.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Alpha Proposal");
        assert_eq!(list[1].name, "Beta Deck");
    }

    #[test]
    fn scan_creates_directory_if_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("nonexistent").join("templates");
        let mut mp = LocalMarketplace::new(&root);
        let count = mp.scan().unwrap();
        assert_eq!(count, 0);
        assert!(root.exists());
    }

    #[test]
    fn scan_skips_invalid_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        let bad = root.join("broken.ktemplate");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("manifest.json"), "{ not valid json }").unwrap();

        let mut mp = LocalMarketplace::new(&root);
        let count = mp.scan().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn filter_by_category() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        write_ktemplate(
            &root.join("deck.ktemplate"),
            &TemplateManifest {
                category: TemplateCategory::PitchDeck,
                ..sample_manifest(id1)
            },
        );
        write_ktemplate(
            &root.join("proposal.ktemplate"),
            &TemplateManifest {
                name: "Proposal".into(),
                category: TemplateCategory::Proposal,
                ..sample_manifest(id2)
            },
        );

        let mut mp = LocalMarketplace::new(&root);
        mp.scan().unwrap();
        let decks = mp.filter_by_category(TemplateCategory::PitchDeck);
        assert_eq!(decks.len(), 1);
        assert_eq!(decks[0].id, id1);
    }

    #[test]
    fn search_matches_name_tag_description() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        write_ktemplate(
            &root.join("deck.ktemplate"),
            &TemplateManifest {
                name: "Sales Deck".into(),
                tags: vec!["sales".into(), "b2b".into()],
                ..sample_manifest(id1)
            },
        );
        write_ktemplate(
            &root.join("onboard.ktemplate"),
            &TemplateManifest {
                name: "Onboarding".into(),
                description: "New employee onboarding slides".into(),
                tags: vec!["hr".into()],
                ..sample_manifest(id2)
            },
        );

        let mut mp = LocalMarketplace::new(&root);
        mp.scan().unwrap();
        assert_eq!(mp.search("sales").len(), 1);
        assert_eq!(mp.search("b2b").len(), 1);
        assert_eq!(mp.search("onboard").len(), 1);
        assert_eq!(mp.search("slides").len(), 1);
        assert_eq!(mp.search("xyz").len(), 0);
    }

    #[test]
    fn install_and_remove_template() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("marketplace");
        let source = tmp.path().join("source.ktemplate");
        let id = Uuid::new_v4();
        write_ktemplate(&source, &sample_manifest(id));

        let mut mp = LocalMarketplace::new(&root);
        mp.scan().unwrap();
        assert_eq!(mp.list().len(), 0);

        let installed = mp.install_local(&source).unwrap();
        assert_eq!(installed.id, id);
        assert_eq!(mp.list().len(), 1);
        assert!(mp.get(id).is_some());

        mp.remove(id).unwrap();
        assert_eq!(mp.list().len(), 0);
        assert!(mp.get(id).is_none());
    }

    #[test]
    fn install_rejects_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("marketplace");
        let source = tmp.path().join("dup.ktemplate");
        let id = Uuid::new_v4();
        write_ktemplate(&source, &sample_manifest(id));

        let mut mp = LocalMarketplace::new(&root);
        mp.scan().unwrap();
        mp.install_local(&source).unwrap();
        let err = mp.install_local(&source).unwrap_err();
        assert!(matches!(err, MarketplaceError::AlreadyInstalled(_)));
    }

    #[test]
    fn remove_nonexistent_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mp = LocalMarketplace::new(tmp.path());
        let err = mp.remove(Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, MarketplaceError::TemplateNotFound(_)));
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let m = sample_manifest(Uuid::new_v4());
        let json = serde_json::to_string(&m).unwrap();
        let parsed: TemplateManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m.id, parsed.id);
        assert_eq!(m.name, parsed.name);
        assert_eq!(m.category, parsed.category);
        assert_eq!(m.tags, parsed.tags);
    }

    #[test]
    fn default_dir_uses_home() {
        let d = LocalMarketplace::default_dir();
        assert!(d.ends_with(".kcreate/templates") || d.ends_with(".kcreate\\templates"));
    }

    fn valid_content_json() -> String {
        // A minimal-but-real template: full-bleed background + a label,
        // matching the wire shape every bundled `content.json` uses.
        r#"{
            "width": 800.0,
            "height": 600.0,
            "items": [
                { "kind": "rect", "parent": null, "x": 0.0, "y": 0.0, "w": 800.0, "h": 600.0,
                  "fill": { "kind": "solid", "r": 0.1, "g": 0.2, "b": 0.3, "a": 1.0 }, "name": "Background" },
                { "kind": "text", "parent": null, "x": 40.0, "y": 80.0, "body": "Imported",
                  "family": "sans-serif", "size": 48.0,
                  "fill": { "kind": "solid", "r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0 }, "name": "Title" }
            ]
        }"#
        .to_string()
    }

    #[test]
    fn import_design_writes_registers_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        let mut mp = LocalMarketplace::new(&root);
        mp.scan().unwrap();
        assert_eq!(mp.list().len(), 0);

        let manifest = mp
            .import_design(ImportSpec {
                name: "My Remixed Poster".into(),
                description: "A remix imported from a file".into(),
                category: TemplateCategory::Poster,
                tags: vec!["remix".into(), "imported".into()],
                page_count: 1,
                content_json: valid_content_json(),
            })
            .unwrap();

        // Registered in memory without a rescan.
        assert_eq!(mp.list().len(), 1);
        assert!(mp.get(manifest.id).is_some());
        assert!(matches!(
            mp.get(manifest.id).unwrap().source,
            Some(TemplateSource::Local { .. })
        ));

        // Folder name is a readable slug + 8-hex id suffix.
        let Some(TemplateSource::Local { path }) = &manifest.source else {
            panic!("expected Local source");
        };
        let dir_name = path.file_name().unwrap().to_str().unwrap();
        assert!(dir_name.starts_with("my-remixed-poster-"));
        assert!(dir_name.ends_with(".ktemplate"));
        assert!(path.join("manifest.json").exists());
        assert!(path.join("content.json").exists());

        // A fresh marketplace pointed at the same root rediscovers it,
        // proving the on-disk entry is a valid library template.
        let mut mp2 = LocalMarketplace::new(&root);
        assert_eq!(mp2.scan().unwrap(), 1);
        let rediscovered = mp2.get(manifest.id).unwrap();
        assert_eq!(rediscovered.name, "My Remixed Poster");
        assert_eq!(rediscovered.category, TemplateCategory::Poster);

        // The persisted content.json parses back into the canonical type.
        let raw = std::fs::read_to_string(path.join("content.json")).unwrap();
        let content: crate::template_library::TemplateContent = serde_json::from_str(&raw).unwrap();
        assert_eq!(content.items.len(), 2);
    }

    #[test]
    fn import_design_rejects_invalid_content_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        let mut mp = LocalMarketplace::new(&root);
        let err = mp
            .import_design(ImportSpec {
                name: "Broken".into(),
                description: String::new(),
                category: TemplateCategory::Custom,
                tags: vec![],
                page_count: 1,
                content_json: "{ not valid json }".into(),
            })
            .unwrap_err();
        assert!(matches!(err, MarketplaceError::InvalidContent(_)));
        // Nothing was written to disk.
        assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none());
        assert_eq!(mp.list().len(), 0);
    }

    #[test]
    fn import_design_same_name_twice_gets_unique_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        let mut mp = LocalMarketplace::new(&root);
        let spec = || ImportSpec {
            name: "Duplicate Name".into(),
            description: "x".into(),
            category: TemplateCategory::Flyer,
            tags: vec![],
            page_count: 1,
            content_json: valid_content_json(),
        };
        let a = mp.import_design(spec()).unwrap();
        let b = mp.import_design(spec()).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(mp.list().len(), 2);
        // A rescan still sees both distinct entries.
        assert_eq!(mp.scan().unwrap(), 2);
    }

    #[test]
    fn import_design_publishes_atomically_leaving_no_staging_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        let mut mp = LocalMarketplace::new(&root);

        let manifest = mp
            .import_design(ImportSpec {
                name: "Atomic Poster".into(),
                description: "x".into(),
                category: TemplateCategory::Poster,
                tags: vec![],
                page_count: 1,
                content_json: valid_content_json(),
            })
            .unwrap();

        // Exactly one entry on disk: the final `.ktemplate/`. The
        // template is only ever observable under its final name, never
        // a half-written `.partial` staging dir, because publish is a
        // single atomic rename.
        let mut ktemplate_dirs = 0usize;
        for entry in std::fs::read_dir(&root).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(!name.ends_with(".partial"), "leftover staging dir: {name}");
            if name.ends_with(".ktemplate") {
                ktemplate_dirs += 1;
            }
        }
        assert_eq!(ktemplate_dirs, 1);

        // The published entry is complete and rediscoverable.
        let Some(TemplateSource::Local { path }) = &manifest.source else {
            panic!("expected Local source");
        };
        assert!(path.join("manifest.json").exists());
        assert!(path.join("content.json").exists());
        let mut mp2 = LocalMarketplace::new(&root);
        assert_eq!(mp2.scan().unwrap(), 1);
    }

    #[test]
    fn scan_sweeps_stale_staging_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");

        // A valid published template alongside crash litter: a staging
        // dir left behind when an import was killed between staging and
        // the atomic rename.
        write_ktemplate(
            &root.join("real-design-abcdef01.ktemplate"),
            &TemplateManifest {
                name: "Real Design".into(),
                ..sample_manifest(Uuid::new_v4())
            },
        );
        let stale = root.join("interrupted-deadbeef.ktemplate.partial");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("manifest.json"), "garbage").unwrap();

        let mut mp = LocalMarketplace::new(&root);
        // Only the valid template counts; the staging dir is invisible.
        assert_eq!(mp.scan().unwrap(), 1);
        // ...and it has been swept from disk so litter can't accumulate.
        assert!(!stale.exists());
        assert!(root.join("real-design-abcdef01.ktemplate").exists());
    }

    #[test]
    fn page_count_clamped_to_at_least_one() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mp = LocalMarketplace::new(tmp.path().join("templates"));
        let m = mp
            .import_design(ImportSpec {
                name: "Zero Pages".into(),
                description: String::new(),
                category: TemplateCategory::Custom,
                tags: vec![],
                page_count: 0,
                content_json: valid_content_json(),
            })
            .unwrap();
        assert_eq!(m.page_count, 1);
    }

    #[test]
    fn slugify_handles_punctuation_and_unicode() {
        assert_eq!(slugify("My Remixed Poster"), "my-remixed-poster");
        assert_eq!(slugify("  Hello,  World!! "), "hello-world");
        assert_eq!(slugify("A/B  C"), "a-b-c");
        assert_eq!(slugify("***"), "imported-design");
        assert_eq!(slugify("日本語"), "imported-design");
        assert_eq!(slugify("café-2024"), "caf-2024");
    }

    #[test]
    fn search_ranks_name_above_tag_above_description() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        // "report" appears in: one name, one tag, one description.
        write_ktemplate(
            &root.join("desc.ktemplate"),
            &TemplateManifest {
                name: "Zulu".into(),
                description: "quarterly report deck".into(),
                tags: vec!["misc".into()],
                ..sample_manifest(Uuid::new_v4())
            },
        );
        write_ktemplate(
            &root.join("tag.ktemplate"),
            &TemplateManifest {
                name: "Yankee".into(),
                description: "nothing here".into(),
                tags: vec!["report".into()],
                ..sample_manifest(Uuid::new_v4())
            },
        );
        write_ktemplate(
            &root.join("name.ktemplate"),
            &TemplateManifest {
                name: "Report Monthly".into(),
                description: "nothing here".into(),
                tags: vec!["misc".into()],
                ..sample_manifest(Uuid::new_v4())
            },
        );

        let mut mp = LocalMarketplace::new(&root);
        mp.scan().unwrap();
        let results = mp.search("report");
        assert_eq!(results.len(), 3, "ranking must not drop matches");
        // name hit first, then tag hit, then description-only hit.
        assert_eq!(results[0].name, "Report Monthly");
        assert_eq!(results[1].name, "Yankee");
        assert_eq!(results[2].name, "Zulu");
    }
}
