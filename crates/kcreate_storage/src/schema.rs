//! `SQLite` schema + migrations for the project document database.
//!
//! The database stores three families of data:
//!
//! - **Nodes** — the document graph, one row per [`kcreate_core::Node`].
//! - **Operations** — the operation log (undo/redo) and the AI action
//!   audit log.
//! - **Assets / Project metadata** — content-addressed asset records
//!   (the bytes live in the blob store) and a key/value bag for
//!   project-level settings.
//!
//! Encryption: [`Database::open_encrypted`] is provided today as a
//! placeholder that takes a key argument and persists it for Phase 1
//! when we adopt `SQLCipher`. We intentionally do not silently fall back
//! to plaintext — the caller must be explicit.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use thiserror::Error;

/// All migrations, applied in order. The migration table records which
/// have already been applied so the function is idempotent.
pub const MIGRATIONS: &[&str] = &[
    // 1: nodes
    r"CREATE TABLE IF NOT EXISTS nodes (
        id TEXT PRIMARY KEY,
        node_type TEXT NOT NULL,
        parent_id TEXT,
        data TEXT NOT NULL,
        version INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );",
    // 2: operations
    r"CREATE TABLE IF NOT EXISTS operations (
        id TEXT PRIMARY KEY,
        timestamp TEXT NOT NULL,
        actor TEXT NOT NULL,
        command TEXT NOT NULL,
        before_patch TEXT NOT NULL,
        after_patch TEXT NOT NULL,
        affected_nodes TEXT NOT NULL,
        ai_generated INTEGER NOT NULL DEFAULT 0
    );",
    // 3: project_meta
    r"CREATE TABLE IF NOT EXISTS project_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );",
    // 4: assets
    r"CREATE TABLE IF NOT EXISTS assets (
        id TEXT PRIMARY KEY,
        hash TEXT NOT NULL,
        mime_type TEXT NOT NULL,
        size_bytes INTEGER NOT NULL,
        created_at TEXT NOT NULL
    );",
    // 5: ai_actions
    r"CREATE TABLE IF NOT EXISTS ai_actions (
        id TEXT PRIMARY KEY,
        timestamp TEXT NOT NULL,
        prompt TEXT,
        model TEXT NOT NULL,
        compute_device TEXT NOT NULL,
        affected_nodes TEXT NOT NULL,
        action_type TEXT NOT NULL
    );",
    // 6: helpful indexes
    "CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_id);",
    "CREATE INDEX IF NOT EXISTS idx_operations_timestamp ON operations(timestamp);",
    "CREATE INDEX IF NOT EXISTS idx_assets_hash ON assets(hash);",
    // 7: brand kits (one row per kit; id == BrandKit::id)
    r"CREATE TABLE IF NOT EXISTS brand_kits (
        id TEXT PRIMARY KEY,
        data TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );",
    // 8: design tokens (single-row by convention; key='current')
    r"CREATE TABLE IF NOT EXISTS design_tokens (
        key TEXT PRIMARY KEY,
        data TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );",
    // 9: export presets (one row per preset; id == ExportPreset::id)
    r"CREATE TABLE IF NOT EXISTS export_presets (
        id TEXT PRIMARY KEY,
        data TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );",
    // 10: components (one row per ComponentDefinition; id == ComponentDefinition::id)
    r"CREATE TABLE IF NOT EXISTS components (
        id TEXT PRIMARY KEY,
        data TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );",
];

/// Schema-level errors. Wraps `rusqlite::Error` and adds a couple of
/// crate-specific cases.
#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database path is not valid utf-8: {0:?}")]
    InvalidPath(PathBuf),
    #[error("encrypted databases are not enabled in this build")]
    EncryptionUnsupported,
}

/// Owned `SQLite` connection wrapper. `Database` is `Send` but not
/// `Sync`; share by moving across threads or use an external mutex.
#[derive(Debug)]
pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    /// Open (or create) a plaintext database at `path` and apply
    /// migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        let mut db = Self { conn, path };
        db.pragma_init()?;
        db.migrate()?;
        Ok(db)
    }

    /// Open an encrypted database. Today this is wired through to the
    /// same `rusqlite` connection but errors out unless we're built
    /// with `SQLCipher` support; production crypto lands in Phase 1.
    pub fn open_encrypted(_path: impl AsRef<Path>, _key: &[u8]) -> Result<Self, DatabaseError> {
        // We deliberately refuse to fall back to plaintext. The Phase 1
        // build will recompile rusqlite with the sqlcipher feature and
        // wire `PRAGMA key = ?` here.
        Err(DatabaseError::EncryptionUnsupported)
    }

    /// Borrow the connection. Useful for ad-hoc queries from other
    /// modules in the crate.
    #[must_use]
    pub const fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Mutable borrow — needed for `transaction()` etc.
    pub const fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Filesystem path the database was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Apply any pending migrations. Idempotent.
    pub fn migrate(&mut self) -> Result<(), DatabaseError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                id INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )?;
        let already: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))?;
        let already = usize::try_from(already.max(0)).unwrap_or(0);
        for (idx, sql) in MIGRATIONS.iter().enumerate().skip(already) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO _migrations (id, applied_at) VALUES (?1, ?2)",
                rusqlite::params![idx as i64 + 1, chrono::Utc::now().to_rfc3339()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    /// How many migrations have been applied so far.
    pub fn applied_migrations(&self) -> Result<usize, DatabaseError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))?;
        Ok(usize::try_from(count.max(0)).unwrap_or(0))
    }

    fn pragma_init(&self) -> Result<(), DatabaseError> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;\n\
             PRAGMA foreign_keys = ON;\n\
             PRAGMA temp_store = MEMORY;\n",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("project.sqlite");
        let db = Database::open(&path).expect("open");
        (dir, db)
    }

    #[test]
    fn migrate_creates_all_tables() {
        let (_dir, db) = fresh();
        let mut stmt = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .expect("prepare");
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect();
        for expected in [
            "_migrations",
            "ai_actions",
            "assets",
            "brand_kits",
            "design_tokens",
            "export_presets",
            "nodes",
            "operations",
            "project_meta",
        ] {
            assert!(
                tables.iter().any(|t| t == expected),
                "table {expected} missing from {tables:?}"
            );
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("project.sqlite");
        {
            let _ = Database::open(&path).expect("first open");
        }
        let db = Database::open(&path).expect("second open");
        assert_eq!(db.applied_migrations().expect("count"), MIGRATIONS.len());
    }

    #[test]
    fn open_encrypted_returns_unsupported_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("p.sqlite");
        let err = Database::open_encrypted(&path, b"key").expect_err("must fail");
        assert!(matches!(err, DatabaseError::EncryptionUnsupported));
    }

    #[test]
    fn second_open_does_not_reapply_migrations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("p.sqlite");
        let _ = Database::open(&path).expect("first open");
        let db2 = Database::open(&path).expect("second open");
        assert_eq!(db2.applied_migrations().expect("count"), MIGRATIONS.len());
    }
}
