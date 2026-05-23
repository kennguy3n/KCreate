//! `.kstudio/` project I/O — the user-visible file format.
//!
//! A `.kstudio/` directory is a *folder package*: it looks like a
//! folder on Linux/Windows and like an opaque file in macOS Finder
//! (via a Bundle bit). Layout:
//!
//! ```text
//! my-design.kstudio/
//!   manifest.json
//!   document.sqlite       <-- nodes, operations, project_meta, assets
//!   blobs/                <-- content-addressed
//!     ab/abcd…ef.blob
//!   thumbnails/           <-- cached page thumbnails (Phase 1+)
//!   exports/              <-- last-export cache (Phase 1+)
//!   ai/                   <-- model-pack outputs / cache (Phase 1+)
//!   cache/                <-- general-purpose, safe to delete
//! ```
//!
//! [`ProjectStore`] owns the database connection and the blob store. It
//! is the canonical persistence layer; everything else in the
//! workspace goes through it.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use kcreate_core::color::ColorSettings;
use kcreate_core::component::ComponentDefinition;
use kcreate_core::document::{DocumentError, DocumentGraph};
use kcreate_core::node::{Node, NodeType};
use kcreate_core::operation::Operation;
use kcreate_core::project::{BrandKit, DesignTokens, ExportPreset};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::blobs::{BlobError, BlobRef, BlobStore};
use crate::schema::{Database, DatabaseError};

const MANIFEST_FORMAT: &str = "kstudio-v1";
const MANIFEST_FILENAME: &str = "manifest.json";
const DATABASE_FILENAME: &str = "document.sqlite";
const BLOBS_DIRNAME: &str = "blobs";
const SUBDIRS: &[&str] = &["thumbnails", "exports", "ai", "cache"];

/// Errors from project I/O.
#[derive(Debug, Error)]
pub enum ProjectStoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] DatabaseError),
    #[error("blob error: {0}")]
    Blob(#[from] BlobError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("uuid parse error: {0}")]
    Uuid(#[from] uuid::Error),
    #[error("document error: {0}")]
    Document(#[from] DocumentError),
    #[error("manifest missing or malformed at {0}")]
    InvalidManifest(PathBuf),
    #[error("project format {0:?} is not supported")]
    UnsupportedFormat(String),
}

/// `manifest.json` schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectManifest {
    /// Schema version of the manifest itself.
    pub version: String,
    /// Human-readable project name.
    pub name: String,
    /// Stable project id.
    pub id: Uuid,
    /// First-created timestamp.
    pub created_at: DateTime<Utc>,
    /// Last-modified timestamp (updated on every save).
    pub modified_at: DateTime<Utc>,
    /// `kstudio-v1`, etc.
    pub format: String,
}

impl ProjectManifest {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            name: name.into(),
            id: Uuid::new_v4(),
            created_at: now,
            modified_at: now,
            format: MANIFEST_FORMAT.into(),
        }
    }
}

/// A `.kstudio/` project on disk.
#[derive(Debug)]
pub struct ProjectStore {
    project_dir: PathBuf,
    manifest: ProjectManifest,
    db: Database,
    blobs: BlobStore,
}

impl ProjectStore {
    /// Create a new `.kstudio/` package at `dir` (which is the
    /// `.kstudio/` directory itself; we don't add the suffix). Fails
    /// if the directory already contains a manifest.
    pub fn create(dir: &Path, name: impl Into<String>) -> Result<Self, ProjectStoreError> {
        let dir = dir.to_path_buf();
        fs::create_dir_all(&dir)?;
        if dir.join(MANIFEST_FILENAME).exists() {
            return Err(ProjectStoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "project already exists at this path",
            )));
        }
        for sub in SUBDIRS {
            fs::create_dir_all(dir.join(sub))?;
        }
        let manifest = ProjectManifest::new(name);
        write_manifest(&dir, &manifest)?;
        let db = Database::open(dir.join(DATABASE_FILENAME))?;
        let blobs = BlobStore::new(dir.join(BLOBS_DIRNAME))?;
        Ok(Self {
            project_dir: dir,
            manifest,
            db,
            blobs,
        })
    }

    /// Open an existing `.kstudio/` package.
    pub fn open(dir: &Path) -> Result<Self, ProjectStoreError> {
        let dir = dir.to_path_buf();
        let manifest_path = dir.join(MANIFEST_FILENAME);
        if !manifest_path.exists() {
            return Err(ProjectStoreError::InvalidManifest(manifest_path));
        }
        let manifest = read_manifest(&dir)?;
        if manifest.format != MANIFEST_FORMAT {
            return Err(ProjectStoreError::UnsupportedFormat(manifest.format));
        }
        for sub in SUBDIRS {
            fs::create_dir_all(dir.join(sub))?;
        }
        let db = Database::open(dir.join(DATABASE_FILENAME))?;
        let blobs = BlobStore::new(dir.join(BLOBS_DIRNAME))?;
        Ok(Self {
            project_dir: dir,
            manifest,
            db,
            blobs,
        })
    }

    /// Path to the `.kstudio/` directory.
    #[must_use]
    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    /// Borrow the project manifest.
    #[must_use]
    pub const fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    /// Borrow the blob store.
    #[must_use]
    pub const fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// Persist the full document graph. Replaces all existing nodes.
    /// For incremental saves use [`Self::save_node`] /
    /// [`Self::delete_node`].
    pub fn save_document(&mut self, doc: &DocumentGraph) -> Result<(), ProjectStoreError> {
        let tx = self.db.conn_mut().transaction()?;
        tx.execute("DELETE FROM nodes", [])?;
        for (_, node) in doc.iter() {
            let data = serde_json::to_string(node)?;
            let parent = node.parent_id.map(|p| p.to_string());
            let node_type = serde_json::to_string(&node.node_type)?;
            tx.execute(
                "INSERT INTO nodes (id, node_type, parent_id, data, version, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    node.id.to_string(),
                    node_type.trim_matches('"'),
                    parent,
                    data,
                    node.version as i64,
                    node.created_at.to_rfc3339(),
                    node.updated_at.to_rfc3339(),
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO project_meta (key, value) VALUES ('root_ids', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(doc.root_ids())?],
        )?;
        tx.commit()?;
        self.touch_modified()?;
        Ok(())
    }

    /// Reconstitute the document graph from the database.
    pub fn load_document(&self) -> Result<DocumentGraph, ProjectStoreError> {
        let mut stmt = self.db.conn().prepare("SELECT data FROM nodes")?;
        let nodes: Vec<Node> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|raw| -> Result<Node, ProjectStoreError> {
                Ok(serde_json::from_str::<Node>(&raw?)?)
            })
            .collect::<Result<_, _>>()?;
        let root_ids: Vec<Uuid> = match self.db.conn().query_row(
            "SELECT value FROM project_meta WHERE key = 'root_ids'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            Ok(s) => serde_json::from_str(&s)?,
            Err(rusqlite::Error::QueryReturnedNoRows) => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(DocumentGraph::from_parts(nodes, root_ids)?)
    }

    /// Append an operation to the operation log table.
    pub fn save_operation(&mut self, op: &Operation) -> Result<(), ProjectStoreError> {
        self.db.conn().execute(
            "INSERT OR REPLACE INTO operations
             (id, timestamp, actor, command, before_patch, after_patch, affected_nodes, ai_generated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                op.id.to_string(),
                op.timestamp.to_rfc3339(),
                op.actor,
                op.command,
                serde_json::to_string(&op.before_patch)?,
                serde_json::to_string(&op.after_patch)?,
                serde_json::to_string(&op.affected_nodes)?,
                i64::from(op.ai_generated),
            ],
        )?;
        Ok(())
    }

    /// Load the most recent `limit` operations, oldest first.
    ///
    /// SQL note: the `operations` table is append-only and grows with
    /// project lifetime (see [`Self::prune_operations`] for the bounded
    /// trim path). A naive `ORDER BY timestamp ASC LIMIT ?1` returns the
    /// *oldest* rows — exactly the wrong half — once the row count
    /// exceeds the limit. The inner subquery picks the newest `limit`
    /// rows; the outer query re-sorts them oldest-first to match the
    /// `OperationLog` push order callers expect.
    pub fn load_operations(&self, limit: usize) -> Result<Vec<Operation>, ProjectStoreError> {
        let mut stmt = self.db.conn().prepare(
            "SELECT id, timestamp, actor, command, before_patch, after_patch, affected_nodes, ai_generated
             FROM (
               SELECT id, timestamp, actor, command, before_patch, after_patch, affected_nodes, ai_generated
               FROM operations
               ORDER BY timestamp DESC
               LIMIT ?1
             ) AS recent
             ORDER BY timestamp ASC",
        )?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                let id: String = row.get(0)?;
                let ts: String = row.get(1)?;
                let actor: String = row.get(2)?;
                let command: String = row.get(3)?;
                let before: String = row.get(4)?;
                let after: String = row.get(5)?;
                let affected: String = row.get(6)?;
                let ai: i64 = row.get(7)?;
                Ok((id, ts, actor, command, before, after, affected, ai))
            })?
            .map(|raw| -> Result<Operation, ProjectStoreError> {
                let (id, ts, actor, command, before, after, affected, ai) = raw?;
                Ok(Operation {
                    id: Uuid::parse_str(&id)?,
                    timestamp: chrono::DateTime::parse_from_rfc3339(&ts)
                        .map_err(|e| ProjectStoreError::Io(std::io::Error::other(e.to_string())))?
                        .with_timezone(&chrono::Utc),
                    actor,
                    command,
                    before_patch: serde_json::from_str(&before)?,
                    after_patch: serde_json::from_str(&after)?,
                    affected_nodes: serde_json::from_str(&affected)?,
                    ai_generated: ai != 0,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Trim the on-disk `operations` table to the `keep` most recent
    /// rows.
    ///
    /// The in-memory `OperationLog` is bounded at `max_depth`. The
    /// on-disk row count tracks that bound by calling this helper from
    /// `project_save` with `keep = max_depth`. The delete is expressed
    /// as `id NOT IN (top-N by timestamp DESC)` so the kept set is
    /// well-defined regardless of how `keep` relates to the row count:
    ///
    /// * `keep >= row_count` — the inner `SELECT` returns every id, so
    ///   `NOT IN (...)` matches nothing and the `DELETE` is a no-op
    ///   (idempotent).
    /// * `keep == 0` — the inner `SELECT` returns the empty set, so
    ///   `NOT IN (...)` matches every row and the table is wiped.
    /// * `0 < keep < row_count` — the inner `SELECT` returns the `keep`
    ///   newest ids; `NOT IN (...)` deletes the remaining `row_count -
    ///   keep` older rows.
    ///
    /// The alternative formulation (`WHERE timestamp <
    /// (SELECT timestamp ... LIMIT 1 OFFSET keep)`) has an off-by-one
    /// at the cutoff row and depends on timestamps being strictly
    /// monotonic across saves; the `NOT IN` form sidesteps both.
    pub fn prune_operations(&mut self, keep: usize) -> Result<usize, ProjectStoreError> {
        // SQLite's bind params want i64; clamp huge `keep` values into
        // the positive i64 range. `keep = 0` is the canonical "wipe"
        // signal.
        let keep_i64 = i64::try_from(keep).unwrap_or(i64::MAX);
        let deleted = self.db.conn().execute(
            "DELETE FROM operations
             WHERE id NOT IN (
               SELECT id FROM operations
               ORDER BY timestamp DESC
               LIMIT ?1
             )",
            params![keep_i64],
        )?;
        Ok(deleted)
    }

    /// Store an asset binary and record it in the `assets` table.
    pub fn store_asset(&mut self, data: &[u8], mime: &str) -> Result<BlobRef, ProjectStoreError> {
        let blob = self.blobs.store(data, mime)?;
        self.db.conn().execute(
            "INSERT OR REPLACE INTO assets (id, hash, mime_type, size_bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                blob.hash,
                blob.mime_type,
                blob.size as i64,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(blob)
    }

    /// Persist a brand kit. `id` matches `BrandKit::id`, so this is a
    /// content-replacing upsert — a second save with the same id wins.
    pub fn save_brand_kit(&mut self, kit: &BrandKit) -> Result<(), ProjectStoreError> {
        self.db.conn().execute(
            "INSERT INTO brand_kits (id, data, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
            params![
                kit.id.to_string(),
                serde_json::to_string(kit)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        self.touch_modified()?;
        Ok(())
    }

    /// Load all brand kits, ordered by `updated_at` ascending so the
    /// most recently edited appears last (the UI shows the list
    /// reversed if it wants most-recent-first).
    pub fn load_brand_kits(&self) -> Result<Vec<BrandKit>, ProjectStoreError> {
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT data FROM brand_kits ORDER BY updated_at ASC")?;
        let kits: Vec<BrandKit> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|raw| -> Result<BrandKit, ProjectStoreError> {
                Ok(serde_json::from_str::<BrandKit>(&raw?)?)
            })
            .collect::<Result<_, _>>()?;
        Ok(kits)
    }

    /// Delete a brand kit by id. Returns the number of rows removed (0 or 1).
    pub fn delete_brand_kit(&mut self, id: Uuid) -> Result<usize, ProjectStoreError> {
        let n = self.db.conn().execute(
            "DELETE FROM brand_kits WHERE id = ?1",
            params![id.to_string()],
        )?;
        if n > 0 {
            self.touch_modified()?;
        }
        Ok(n)
    }

    /// Persist the singleton design tokens bag. There is exactly one
    /// row in this table; the `key` column is `'current'`.
    pub fn save_design_tokens(&mut self, tokens: &DesignTokens) -> Result<(), ProjectStoreError> {
        self.db.conn().execute(
            "INSERT INTO design_tokens (key, data, updated_at) VALUES ('current', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
            params![serde_json::to_string(tokens)?, Utc::now().to_rfc3339()],
        )?;
        self.touch_modified()?;
        Ok(())
    }

    /// Load the design tokens. Returns `DesignTokens::default()` when
    /// nothing has been persisted yet (fresh projects).
    pub fn load_design_tokens(&self) -> Result<DesignTokens, ProjectStoreError> {
        match self.db.conn().query_row(
            "SELECT data FROM design_tokens WHERE key = 'current'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            Ok(s) => Ok(serde_json::from_str::<DesignTokens>(&s)?),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(DesignTokens::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist the singleton document-level color management
    /// settings (working spaces, rendering intent, soft-proof). There
    /// is exactly one row in this table; the `key` column is
    /// `'current'`.
    pub fn save_color_settings(
        &mut self,
        settings: &ColorSettings,
    ) -> Result<(), ProjectStoreError> {
        self.db.conn().execute(
            "INSERT INTO color_settings (key, data, updated_at) VALUES ('current', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
            params![serde_json::to_string(settings)?, Utc::now().to_rfc3339()],
        )?;
        self.touch_modified()?;
        Ok(())
    }

    /// Load the color settings. Returns `ColorSettings::default()`
    /// (sRGB working space, no CMYK profile, perceptual intent) when
    /// nothing has been persisted yet — older projects predating the
    /// Phase 2 CMYK foundation transparently get the sRGB default.
    pub fn load_color_settings(&self) -> Result<ColorSettings, ProjectStoreError> {
        match self.db.conn().query_row(
            "SELECT data FROM color_settings WHERE key = 'current'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            Ok(s) => Ok(serde_json::from_str::<ColorSettings>(&s)?),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(ColorSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist a single export preset. Upsert keyed on `ExportPreset::id`.
    pub fn save_export_preset(&mut self, preset: &ExportPreset) -> Result<(), ProjectStoreError> {
        self.db.conn().execute(
            "INSERT INTO export_presets (id, data, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
            params![
                preset.id.to_string(),
                serde_json::to_string(preset)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        self.touch_modified()?;
        Ok(())
    }

    /// Load every export preset, ordered by creation time (`updated_at` ASC).
    pub fn load_export_presets(&self) -> Result<Vec<ExportPreset>, ProjectStoreError> {
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT data FROM export_presets ORDER BY updated_at ASC")?;
        let presets: Vec<ExportPreset> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|raw| -> Result<ExportPreset, ProjectStoreError> {
                Ok(serde_json::from_str::<ExportPreset>(&raw?)?)
            })
            .collect::<Result<_, _>>()?;
        Ok(presets)
    }

    /// Delete an export preset by id. Returns the number of rows removed.
    pub fn delete_export_preset(&mut self, id: Uuid) -> Result<usize, ProjectStoreError> {
        let n = self.db.conn().execute(
            "DELETE FROM export_presets WHERE id = ?1",
            params![id.to_string()],
        )?;
        if n > 0 {
            self.touch_modified()?;
        }
        Ok(n)
    }

    /// Persist a single component definition. Upsert keyed on
    /// `ComponentDefinition::id`.
    pub fn save_component(
        &mut self,
        component: &ComponentDefinition,
    ) -> Result<(), ProjectStoreError> {
        self.db.conn().execute(
            "INSERT INTO components (id, data, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
            params![
                component.id.to_string(),
                serde_json::to_string(component)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        self.touch_modified()?;
        Ok(())
    }

    /// Bulk-persist a set of components. Rows missing from the input
    /// are deleted (the in-memory map is the source of truth).
    pub fn replace_components(
        &mut self,
        components: &std::collections::HashMap<Uuid, ComponentDefinition>,
    ) -> Result<(), ProjectStoreError> {
        let tx = self.db.conn_mut().transaction()?;
        tx.execute("DELETE FROM components", [])?;
        {
            let mut stmt =
                tx.prepare("INSERT INTO components (id, data, updated_at) VALUES (?1, ?2, ?3)")?;
            for component in components.values() {
                stmt.execute(params![
                    component.id.to_string(),
                    serde_json::to_string(component)?,
                    Utc::now().to_rfc3339(),
                ])?;
            }
        }
        tx.commit()?;
        self.touch_modified()?;
        Ok(())
    }

    /// Load every component definition into the canonical in-memory
    /// `HashMap` keyed by id.
    pub fn load_components(
        &self,
    ) -> Result<std::collections::HashMap<Uuid, ComponentDefinition>, ProjectStoreError> {
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT data FROM components ORDER BY updated_at ASC")?;
        let mut out = std::collections::HashMap::new();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for raw in rows {
            let def: ComponentDefinition = serde_json::from_str(&raw?)?;
            out.insert(def.id, def);
        }
        Ok(out)
    }

    /// Delete a component definition by id. Returns the number of
    /// rows removed (0 or 1).
    pub fn delete_component(&mut self, id: Uuid) -> Result<usize, ProjectStoreError> {
        let n = self.db.conn().execute(
            "DELETE FROM components WHERE id = ?1",
            params![id.to_string()],
        )?;
        if n > 0 {
            self.touch_modified()?;
        }
        Ok(n)
    }

    /// Bump `modified_at` and persist.
    fn touch_modified(&mut self) -> Result<(), ProjectStoreError> {
        self.manifest.modified_at = Utc::now();
        write_manifest(&self.project_dir, &self.manifest)?;
        Ok(())
    }
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(MANIFEST_FILENAME)
}

fn write_manifest(dir: &Path, manifest: &ProjectManifest) -> Result<(), ProjectStoreError> {
    let json = serde_json::to_string_pretty(manifest)?;
    let tmp = manifest_path(dir).with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, manifest_path(dir))?;
    Ok(())
}

fn read_manifest(dir: &Path) -> Result<ProjectManifest, ProjectStoreError> {
    let raw = fs::read_to_string(manifest_path(dir))?;
    Ok(serde_json::from_str(&raw)?)
}

// Helper for tests / external users: classify a node type as container/leaf.
#[must_use]
pub const fn is_container(node_type: NodeType) -> bool {
    node_type.is_container()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{Bounds, Node, NodeType};

    fn new_project() -> (tempfile::TempDir, ProjectStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("My.kstudio");
        let store = ProjectStore::create(&p, "My").expect("create");
        (dir, store)
    }

    #[test]
    fn create_writes_manifest_and_subdirs() {
        let (_dir, store) = new_project();
        let m = store.manifest();
        assert_eq!(m.format, MANIFEST_FORMAT);
        assert_eq!(m.name, "My");
        for sub in SUBDIRS {
            assert!(store.project_dir().join(sub).is_dir());
        }
        assert!(store.project_dir().join(MANIFEST_FILENAME).exists());
        assert!(store.project_dir().join(DATABASE_FILENAME).exists());
    }

    #[test]
    fn open_existing_project() {
        let (_dir, store) = new_project();
        let path = store.project_dir().to_path_buf();
        drop(store);
        let reopened = ProjectStore::open(&path).expect("reopen");
        assert_eq!(reopened.manifest().name, "My");
    }

    #[test]
    fn open_missing_manifest_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = ProjectStore::open(dir.path()).expect_err("must err");
        assert!(matches!(err, ProjectStoreError::InvalidManifest(_)));
    }

    #[test]
    fn save_and_load_document_roundtrip() {
        let (_dir, mut store) = new_project();
        let mut doc = DocumentGraph::new();
        let mut page = Node::new(NodeType::Page, "Page 1");
        page.bounds = Bounds::new(0.0, 0.0, 1920.0, 1080.0);
        let page_id = page.id;
        doc.insert_node(page).expect("insert page");
        let mut child = Node::new(NodeType::VectorLayer, "Rect");
        child.parent_id = Some(page_id);
        child.bounds = Bounds::new(10.0, 10.0, 100.0, 50.0);
        doc.insert_node(child).expect("insert child");

        store.save_document(&doc).expect("save");
        let loaded = store.load_document().expect("load");
        assert_eq!(loaded.node_count(), 2);
        assert_eq!(loaded.root_ids(), &[page_id]);
        assert_eq!(loaded.children_of(page_id).len(), 1);
    }

    #[test]
    fn asset_store_round_trip() {
        let (_dir, mut store) = new_project();
        let bytes = b"some-png-bytes";
        let r = store.store_asset(bytes, "image/png").expect("store");
        assert_eq!(r.size, bytes.len() as u64);
        let loaded = store.blobs().load(&r.hash).expect("load");
        assert_eq!(loaded, bytes);
    }

    #[test]
    fn save_operation_round_trip() {
        let (_dir, mut store) = new_project();
        let op = Operation::new(
            "user",
            "noop",
            serde_json::json!({}),
            serde_json::json!({}),
            Vec::new(),
        );
        store.save_operation(&op).expect("save");
        let ops = store.load_operations(10).expect("load");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].id, op.id);
        assert_eq!(ops[0].actor, "user");
    }

    /// Regression: `load_operations(limit)` must return the *newest*
    /// `limit` rows, not the oldest, once the table grows past `limit`.
    /// Without the subquery-then-reverse trick, the SQL
    /// `ORDER BY timestamp ASC LIMIT ?1` would return the very first
    /// `limit` ops ever pushed and silently lose the user's recent
    /// undo history on project reopen.
    #[test]
    fn load_operations_returns_most_recent_when_over_limit() {
        let (_dir, mut store) = new_project();
        // Insert 5 ops with controllable timestamps so the ordering is
        // unambiguous regardless of clock resolution.
        let mut ids = Vec::new();
        for n in 0..5 {
            let mut op = Operation::new(
                "user",
                format!("op-{n}"),
                serde_json::json!({}),
                serde_json::json!({}),
                Vec::new(),
            );
            op.timestamp =
                chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000 + i64::from(n), 0)
                    .expect("timestamp");
            store.save_operation(&op).expect("save");
            ids.push(op.id);
        }
        // Ask for only 3 — must return ops #2, #3, #4 (oldest-first).
        let ops = store.load_operations(3).expect("load");
        let got: Vec<uuid::Uuid> = ops.iter().map(|o| o.id).collect();
        let want = vec![ids[2], ids[3], ids[4]];
        assert_eq!(
            got, want,
            "load_operations must select the most-recent rows, then sort oldest-first"
        );
    }

    /// Bounded on-disk operation log. `prune_operations(keep)` removes
    /// every row older than the `keep`-th most recent, leaving the
    /// table at exactly `keep` rows (or fewer if it started smaller).
    /// The kept rows must still be the *newest* ones.
    #[test]
    fn prune_operations_keeps_most_recent() {
        let (_dir, mut store) = new_project();
        let mut ids = Vec::new();
        for n in 0..10 {
            let mut op = Operation::new(
                "user",
                format!("op-{n}"),
                serde_json::json!({}),
                serde_json::json!({}),
                Vec::new(),
            );
            op.timestamp =
                chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000 + i64::from(n), 0)
                    .expect("timestamp");
            store.save_operation(&op).expect("save");
            ids.push(op.id);
        }
        let removed = store.prune_operations(4).expect("prune");
        assert_eq!(removed, 6, "must remove the 6 oldest rows");
        let remaining = store.load_operations(100).expect("load after prune");
        let got: Vec<uuid::Uuid> = remaining.iter().map(|o| o.id).collect();
        assert_eq!(got, vec![ids[6], ids[7], ids[8], ids[9]]);

        // Idempotency: re-pruning with the same budget is a no-op.
        let removed_again = store.prune_operations(4).expect("reprune");
        assert_eq!(removed_again, 0);

        // keep=0 wipes the table.
        let wiped = store.prune_operations(0).expect("wipe");
        assert_eq!(wiped, 4);
        assert!(store.load_operations(100).expect("after wipe").is_empty());
    }

    #[test]
    fn create_in_existing_project_dir_errors() {
        let (_dir, store) = new_project();
        let p = store.project_dir().to_path_buf();
        drop(store);
        let err = ProjectStore::create(&p, "Other").expect_err("must err");
        assert!(matches!(err, ProjectStoreError::Io(_)));
    }

    #[test]
    fn brand_kit_round_trip_survives_close_and_reopen() {
        use kcreate_core::node::RgbaColor;
        use kcreate_core::project::{BrandKit, NamedColor};
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("My.kstudio");
        let mut store = ProjectStore::create(&p, "My").expect("create");

        let mut kit = BrandKit::new("KChat brand");
        kit.colors.push(NamedColor {
            name: "primary".into(),
            color: RgbaColor::new(0.486, 0.227, 0.929, 1.0),
        });
        let kit_id = kit.id;
        store.save_brand_kit(&kit).expect("save kit");

        drop(store);
        let reopened = ProjectStore::open(&p).expect("reopen");
        let kits = reopened.load_brand_kits().expect("load kits");
        assert_eq!(kits.len(), 1, "exactly one kit persisted");
        assert_eq!(kits[0].id, kit_id);
        assert_eq!(kits[0].name, "KChat brand");
        assert_eq!(kits[0].colors[0].name, "primary");
    }

    #[test]
    fn design_tokens_round_trip_survives_reopen() {
        use kcreate_core::node::RgbaColor;
        use kcreate_core::project::DesignTokens;
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("My.kstudio");
        let mut store = ProjectStore::create(&p, "My").expect("create");

        // Default-empty load before any save.
        let initial = store.load_design_tokens().expect("default load");
        assert!(initial.colors.is_empty());

        let mut tokens = DesignTokens::default();
        tokens.colors.insert(
            "brand/primary".into(),
            RgbaColor::new(0.486, 0.227, 0.929, 1.0),
        );
        tokens.spacing.insert("space/4".into(), 16.0);
        store.save_design_tokens(&tokens).expect("save");

        drop(store);
        let reopened = ProjectStore::open(&p).expect("reopen");
        let loaded = reopened.load_design_tokens().expect("load");
        assert_eq!(loaded.colors.len(), 1);
        assert!(loaded.colors.contains_key("brand/primary"));
        assert_eq!(loaded.spacing.get("space/4").copied(), Some(16.0));
    }

    #[test]
    fn export_preset_round_trip_and_delete() {
        use kcreate_core::project::{ExportFormat, ExportPreset};
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("My.kstudio");
        let mut store = ProjectStore::create(&p, "My").expect("create");

        let one = ExportPreset::new("PNG @1x", ExportFormat::Png, 1.0);
        let two = ExportPreset::new("PNG @2x", ExportFormat::Png, 2.0);
        store.save_export_preset(&one).expect("save 1x");
        store.save_export_preset(&two).expect("save 2x");

        let presets = store.load_export_presets().expect("load");
        assert_eq!(presets.len(), 2);
        let ids: Vec<_> = presets.iter().map(|p| p.id).collect();
        assert!(ids.contains(&one.id));
        assert!(ids.contains(&two.id));

        // Delete one.
        let removed = store.delete_export_preset(one.id).expect("delete");
        assert_eq!(removed, 1);
        let after = store.load_export_presets().expect("load after delete");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, two.id);

        // Deleting an unknown id is a no-op.
        let missing = store
            .delete_export_preset(Uuid::new_v4())
            .expect("delete missing");
        assert_eq!(missing, 0);
    }

    #[test]
    fn component_round_trip_and_replace() {
        use kcreate_core::component::{ComponentDefinition, ComponentVariant};
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("My.kstudio");
        let mut store = ProjectStore::create(&p, "My").expect("create");

        let mut def = ComponentDefinition::new("Button");
        let _ = def.add_variant(ComponentVariant::new("Hover"));
        let cid = def.id;
        store.save_component(&def).expect("save");
        let loaded = store.load_components().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get(&cid).expect("present").variants.len(), 2);

        // replace_components mirrors the in-memory map and drops
        // missing rows. Build a fresh map containing only a new
        // definition, then verify the old one is gone.
        let mut fresh = std::collections::HashMap::new();
        let other = ComponentDefinition::new("Card");
        let other_id = other.id;
        fresh.insert(other.id, other);
        store.replace_components(&fresh).expect("replace");
        let after = store.load_components().expect("load after");
        assert_eq!(after.len(), 1);
        assert!(after.contains_key(&other_id));
        assert!(!after.contains_key(&cid));

        // Delete a missing id is a no-op.
        let removed = store.delete_component(Uuid::new_v4()).expect("noop");
        assert_eq!(removed, 0);
        // Delete the real one and check.
        let removed = store.delete_component(other_id).expect("delete");
        assert_eq!(removed, 1);
        let empty = store.load_components().expect("empty");
        assert!(empty.is_empty());

        // Round-trip through close+reopen.
        store.save_component(&def).expect("save again");
        drop(store);
        let reopened = ProjectStore::open(&p).expect("reopen");
        let loaded = reopened.load_components().expect("load after reopen");
        assert!(loaded.contains_key(&cid));
    }
}
