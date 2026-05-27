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

    /// Search templates by name or tag (case-insensitive substring).
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&TemplateManifest> {
        let q = query.to_lowercase();
        let mut v: Vec<_> = self
            .templates
            .values()
            .filter(|t| {
                t.name.to_lowercase().contains(&q)
                    || t.tags.iter().any(|tag| tag.to_lowercase().contains(&q))
                    || t.description.to_lowercase().contains(&q)
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
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
}
