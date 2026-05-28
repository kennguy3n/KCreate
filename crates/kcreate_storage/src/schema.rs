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
    // 11: color settings (single-row by convention; key='current').
    //     Phase 2 CMYK / ICC color management foundation. The `data`
    //     column stores a serialized `kcreate_core::color::ColorSettings`
    //     JSON blob (working RGB / CMYK spaces, rendering intent,
    //     soft-proof profile, gamut warning toggle).
    r"CREATE TABLE IF NOT EXISTS color_settings (
        key TEXT PRIMARY KEY,
        data TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );",
    // 12: undo grouping (Phase 6 Task 15). `group_id` ties together
    //     ops that should undo as one user-facing action — e.g. a
    //     drag-to-move sequence is recorded as 50 tiny move ops in
    //     `operations` but a single click on Undo collapses them.
    //     Backwards-compatible: existing rows have NULL `group_id`
    //     (= no grouping) and the column is read into
    //     `Operation::group_id: Option<Uuid>`.
    "ALTER TABLE operations ADD COLUMN group_id TEXT;",
    // 13: design-review annotations (Phase 8 Task 4). One row per
    //     annotation; replies hang off a thread_id. Position is
    //     stored as two REAL columns so a future query can find
    //     "all annotations near (x, y)" with an index without
    //     decoding JSON.
    r"CREATE TABLE IF NOT EXISTS annotations (
        id TEXT PRIMARY KEY,
        page_id TEXT NOT NULL,
        author_peer_id TEXT NOT NULL,
        author_name TEXT NOT NULL,
        position_x REAL NOT NULL,
        position_y REAL NOT NULL,
        text TEXT NOT NULL,
        timestamp TEXT NOT NULL,
        resolved INTEGER NOT NULL DEFAULT 0,
        thread_id TEXT
    );",
    "CREATE INDEX IF NOT EXISTS idx_annotations_page ON annotations(page_id);",
    "CREATE INDEX IF NOT EXISTS idx_annotations_thread ON annotations(thread_id);",
    // 14: brand-kit versioning (Phase 8 Task 15). Each row is a
    //     snapshot of a `BrandKit` at a point in time. `snapshot`
    //     is the full serialized JSON so a restore is a single
    //     row read.
    r"CREATE TABLE IF NOT EXISTS brand_kit_versions (
        version_id TEXT PRIMARY KEY,
        brand_kit_id TEXT NOT NULL,
        timestamp TEXT NOT NULL,
        description TEXT NOT NULL,
        snapshot TEXT NOT NULL
    );",
    "CREATE INDEX IF NOT EXISTS idx_brand_kit_versions_kit ON brand_kit_versions(brand_kit_id, timestamp);",
    // 15: per-project encryption configuration (Phase 8 Task 25).
    //     Stores the PBKDF2 salt + iteration count so an existing
    //     project can be unlocked without bundling the salt in the
    //     manifest. `encryption_kdf_salt` is 16 raw bytes encoded
    //     as base64. Plaintext projects do not write into this
    //     table; presence of any row implies "this project is
    //     encrypted".
    r"CREATE TABLE IF NOT EXISTS encryption_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
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
    #[error("supplied passphrase did not match the database key")]
    EncryptionWrongKey,
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

    /// Open (or create) an encrypted database at `path` with the
    /// supplied raw key.
    ///
    /// `key` should be the 32-byte output of a PBKDF2-SHA256
    /// derivation over the user's passphrase (see
    /// `kcreate_storage::crypto::derive_key`). Plaintext fallback
    /// is intentionally disallowed — pass an empty key to get a
    /// hard failure rather than a silent downgrade.
    ///
    /// The connection is configured with `PRAGMA key` first
    /// (SQLCipher requires the key to be set before any other
    /// statement on the connection), then a sentinel query is
    /// executed to validate the key against an existing
    /// ciphertext. Wrong keys map to
    /// [`DatabaseError::EncryptionWrongKey`].
    pub fn open_encrypted(path: impl AsRef<Path>, key: &[u8]) -> Result<Self, DatabaseError> {
        if key.is_empty() {
            return Err(DatabaseError::EncryptionUnsupported);
        }
        let path = path.as_ref().to_path_buf();
        let existed_before = path.exists();
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        // Set the key in raw form using the x'...' hex literal so
        // SQLCipher uses the bytes verbatim instead of running its
        // own PBKDF2 over a passphrase. The caller has already
        // derived a 32-byte key from the user's passphrase.
        let hex_key = hex::encode(key);
        conn.pragma_update(None, "key", format!("x'{hex_key}'"))?;
        // Defence-in-depth: probe `PRAGMA cipher_version` to confirm
        // we're actually talking to a SQLCipher-capable build. If a
        // future Cargo.toml refactor drops the sqlcipher feature
        // this surfaces immediately rather than at a later
        // `sqlcipher_export` call.
        assert_sqlcipher_available(&conn)?;
        // Sanity-check the key by reading the schema. On wrong
        // key, SQLCipher returns SQLITE_NOTADB at either prepare
        // or query time depending on the SQLCipher build; treat
        // both as a wrong-key signal.
        if existed_before {
            validate_encrypted_open(&conn)?;
        }
        let mut db = Self { conn, path };
        db.pragma_init()?;
        db.migrate()?;
        Ok(db)
    }

    /// Encrypt an existing plaintext database in place.
    ///
    /// SQLCipher's `sqlcipher_export` requires creating a side
    /// database and copying schema + data, then atomically
    /// replacing the original. Returns the path the encrypted
    /// database lives at (always equal to the input `path`).
    ///
    /// The plaintext `path` must already exist; calling this on a
    /// missing database is an error.
    pub fn encrypt_existing(path: impl AsRef<Path>, key: &[u8]) -> Result<PathBuf, DatabaseError> {
        if key.is_empty() {
            return Err(DatabaseError::EncryptionUnsupported);
        }
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Err(DatabaseError::Sqlite(rusqlite::Error::InvalidPath(path)));
        }
        let tmp = path.with_extension("encrypted.tmp");
        if tmp.exists() {
            std::fs::remove_file(&tmp).map_err(|e| {
                DatabaseError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?;
        }
        // Open the source as plaintext and ATTACH the destination
        // with a fresh key, then sqlcipher_export.
        let conn = Connection::open(&path)?;
        // Probe for SQLCipher support before issuing the
        // SQLCipher-specific `ATTACH … KEY` / `sqlcipher_export`
        // statements so the error message is the user-meaningful
        // "encryption not enabled in this build" rather than
        // "no such function: sqlcipher_export".
        assert_sqlcipher_available(&conn)?;
        let hex_key = hex::encode(key);
        let tmp_str = tmp
            .to_str()
            .ok_or_else(|| DatabaseError::InvalidPath(tmp.clone()))?;
        // `ATTACH DATABASE … KEY` is the SQLCipher-specific syntax
        // for opening the side database with an encryption key.
        conn.execute(
            &format!("ATTACH DATABASE ?1 AS encrypted KEY \"x'{hex_key}'\""),
            rusqlite::params![tmp_str],
        )?;
        conn.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
        conn.execute("DETACH DATABASE encrypted", [])?;
        drop(conn);
        std::fs::rename(&tmp, &path).map_err(|e| {
            DatabaseError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })?;
        Ok(path)
    }

    /// Re-key an existing encrypted database without rewriting the
    /// pages. Uses SQLCipher's `PRAGMA rekey` which re-encrypts
    /// the database header in place.
    pub fn change_key(
        path: impl AsRef<Path>,
        old_key: &[u8],
        new_key: &[u8],
    ) -> Result<(), DatabaseError> {
        if old_key.is_empty() || new_key.is_empty() {
            return Err(DatabaseError::EncryptionUnsupported);
        }
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        // Probe for SQLCipher support before touching `PRAGMA key`
        // / `PRAGMA rekey` so a build without sqlcipher fails with
        // the explicit `EncryptionUnsupported` error.
        assert_sqlcipher_available(&conn)?;
        let hex_old = hex::encode(old_key);
        conn.pragma_update(None, "key", format!("x'{hex_old}'"))?;
        // Validate the old key actually opens the database; without
        // this an attacker-supplied "old_key" silently re-keys to
        // the new one against a plaintext or wrong-keyed db.
        validate_encrypted_open(&conn)?;
        let hex_new = hex::encode(new_key);
        conn.pragma_update(None, "rekey", format!("x'{hex_new}'"))?;
        Ok(())
    }

    /// Decrypt an existing encrypted database into a new plaintext
    /// path. Returns the plaintext path. Used by the "export
    /// unencrypted copy" recovery flow.
    pub fn export_plaintext(
        path: impl AsRef<Path>,
        key: &[u8],
        plaintext_path: impl AsRef<Path>,
    ) -> Result<PathBuf, DatabaseError> {
        if key.is_empty() {
            return Err(DatabaseError::EncryptionUnsupported);
        }
        let path = path.as_ref().to_path_buf();
        let plain = plaintext_path.as_ref().to_path_buf();
        if plain.exists() {
            std::fs::remove_file(&plain).map_err(|e| {
                DatabaseError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?;
        }
        let conn = Connection::open(&path)?;
        assert_sqlcipher_available(&conn)?;
        let hex_key = hex::encode(key);
        conn.pragma_update(None, "key", format!("x'{hex_key}'"))?;
        // Validate the key opened the database successfully.
        validate_encrypted_open(&conn)?;
        let plain_str = plain
            .to_str()
            .ok_or_else(|| DatabaseError::InvalidPath(plain.clone()))?;
        // ATTACH with empty key produces a plaintext side database.
        conn.execute(
            "ATTACH DATABASE ?1 AS plaintext KEY ''",
            rusqlite::params![plain_str],
        )?;
        conn.query_row("SELECT sqlcipher_export('plaintext')", [], |_| Ok(()))?;
        conn.execute("DETACH DATABASE plaintext", [])?;
        Ok(plain)
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

/// Run a sentinel query against an encrypted connection that has
/// just had `PRAGMA key = …` applied. Returns
/// [`DatabaseError::EncryptionWrongKey`] on SQLCipher's
/// `SQLITE_NOTADB` (raised at either prepare or query time
/// depending on the SQLCipher build), forwards every other SQLite
/// error verbatim.
fn validate_encrypted_open(conn: &Connection) -> Result<(), DatabaseError> {
    let prepared = conn.prepare("SELECT count(*) FROM sqlite_master");
    let mut stmt = match prepared {
        Ok(stmt) => stmt,
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::NotADatabase =>
        {
            return Err(DatabaseError::EncryptionWrongKey);
        }
        Err(e) => return Err(DatabaseError::Sqlite(e)),
    };
    match stmt.query_row([], |r| r.get::<_, i64>(0)) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::NotADatabase =>
        {
            Err(DatabaseError::EncryptionWrongKey)
        }
        Err(e) => Err(DatabaseError::Sqlite(e)),
    }
}

/// Verify the underlying `rusqlite` build actually links a
/// SQLCipher-capable SQLite. `PRAGMA cipher_version` is a
/// SQLCipher extension; on a plain SQLite build the pragma returns
/// no row (or an empty string), which we map to
/// [`DatabaseError::EncryptionUnsupported`] so the caller gets a
/// hard, explicit failure instead of a silent downgrade to
/// plaintext or a confusing "no such function: sqlcipher_export"
/// at a later step.
///
/// Defence-in-depth: the workspace `Cargo.toml` requests
/// `rusqlite/bundled-sqlcipher-vendored-openssl`, but a future
/// refactor could accidentally drop that feature without anyone
/// noticing until production. This runtime probe catches that
/// regression on the very first encrypted-database operation.
fn assert_sqlcipher_available(conn: &Connection) -> Result<(), DatabaseError> {
    let version: Option<String> = conn
        .query_row("PRAGMA cipher_version", [], |r| r.get::<_, String>(0))
        .ok();
    match version {
        Some(v) if !v.trim().is_empty() => Ok(()),
        _ => Err(DatabaseError::EncryptionUnsupported),
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
            "color_settings",
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
    fn open_encrypted_with_empty_key_rejects() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("p.sqlite");
        let err = Database::open_encrypted(&path, &[]).expect_err("must fail");
        assert!(matches!(err, DatabaseError::EncryptionUnsupported));
    }

    #[test]
    fn open_encrypted_round_trip_writes_then_reads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("enc.sqlite");
        let key = [0xA5u8; 32];
        {
            let db = Database::open_encrypted(&path, &key).expect("open new");
            db.conn()
                .execute("CREATE TABLE t (v INTEGER)", [])
                .expect("create");
            db.conn()
                .execute("INSERT INTO t VALUES (42)", [])
                .expect("insert");
        }
        let db = Database::open_encrypted(&path, &key).expect("reopen");
        let v: i64 = db
            .conn()
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .expect("select");
        assert_eq!(v, 42);
    }

    #[test]
    fn open_encrypted_with_wrong_key_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("enc.sqlite");
        let good = [0xA5u8; 32];
        let bad = [0xB6u8; 32];
        {
            let _db = Database::open_encrypted(&path, &good).expect("create");
        }
        let err = Database::open_encrypted(&path, &bad).expect_err("wrong key");
        assert!(
            matches!(err, DatabaseError::EncryptionWrongKey),
            "got {err:?}"
        );
    }

    #[test]
    fn encrypt_existing_and_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plain.sqlite");
        {
            let db = Database::open(&path).expect("open plain");
            db.conn()
                .execute("CREATE TABLE t (v INTEGER)", [])
                .expect("create");
            db.conn()
                .execute("INSERT INTO t VALUES (7)", [])
                .expect("insert");
        }
        let key = [0xC3u8; 32];
        Database::encrypt_existing(&path, &key).expect("encrypt");
        let db = Database::open_encrypted(&path, &key).expect("reopen encrypted");
        let v: i64 = db
            .conn()
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .expect("select");
        assert_eq!(v, 7);
    }

    #[test]
    fn change_key_re_keys_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("enc.sqlite");
        let old = [0xD1u8; 32];
        let new = [0xE2u8; 32];
        {
            let db = Database::open_encrypted(&path, &old).expect("create");
            db.conn()
                .execute("CREATE TABLE t (v INTEGER)", [])
                .expect("create");
            db.conn()
                .execute("INSERT INTO t VALUES (3)", [])
                .expect("insert");
        }
        Database::change_key(&path, &old, &new).expect("rekey");
        let err = Database::open_encrypted(&path, &old).expect_err("old must fail");
        assert!(
            matches!(err, DatabaseError::EncryptionWrongKey),
            "got {err:?}"
        );
        let db = Database::open_encrypted(&path, &new).expect("new key opens");
        let v: i64 = db
            .conn()
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .expect("select");
        assert_eq!(v, 3);
    }

    #[test]
    fn export_plaintext_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let enc = dir.path().join("enc.sqlite");
        let plain = dir.path().join("plain.sqlite");
        let key = [0xFFu8; 32];
        {
            let db = Database::open_encrypted(&enc, &key).expect("create");
            db.conn()
                .execute("CREATE TABLE t (v INTEGER)", [])
                .expect("create");
            db.conn()
                .execute("INSERT INTO t VALUES (99)", [])
                .expect("insert");
        }
        Database::export_plaintext(&enc, &key, &plain).expect("export");
        let db = Database::open(&plain).expect("open plain");
        let v: i64 = db
            .conn()
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .expect("select");
        assert_eq!(v, 99);
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
