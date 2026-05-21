//! Content-addressed blob store, BLAKE3-hashed.
//!
//! Layout (relative to `base_dir`):
//!
//! ```text
//! blobs/
//!   ab/
//!     ab[…]9f.blob   <-- contents
//! ```
//!
//! The two-char prefix is the first two hex chars of the BLAKE3 hash;
//! splitting into shards keeps directory entries cheap on filesystems
//! that perform poorly with very large directories (ext4, NTFS,
//! HFS+/APFS).

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Blob-store errors.
#[derive(Debug, Error)]
pub enum BlobError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("blob hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("blob not found: {0}")]
    NotFound(String),
}

/// A reference to a stored blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    /// Lowercase hex BLAKE3 hash (64 chars).
    pub hash: String,
    /// Length of the blob in bytes.
    pub size: u64,
    /// Absolute path to the blob file on disk.
    pub path: PathBuf,
    /// MIME type provided by the caller. Not validated.
    pub mime_type: String,
}

/// On-disk blob store rooted at `base_dir`.
#[derive(Debug, Clone)]
pub struct BlobStore {
    base_dir: PathBuf,
}

impl BlobStore {
    /// Create or open a blob store rooted at `base_dir`. Creates the
    /// directory if missing.
    pub fn new(base_dir: impl Into<PathBuf>) -> io::Result<Self> {
        let base_dir = base_dir.into();
        fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    /// Directory containing the blob tree.
    #[must_use]
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Hash `data` and write to `<base>/<aa>/<full>.blob`. If the blob
    /// already exists (deduplication), no disk write is performed. The
    /// returned [`BlobRef`] is stable across stores of identical
    /// content.
    pub fn store(&self, data: &[u8], mime_type: impl Into<String>) -> Result<BlobRef, BlobError> {
        let hash_hex = blake3::hash(data).to_hex().to_string();
        let path = self.path_for(&hash_hex);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            // Write to a temp file then rename for atomicity.
            let tmp = path.with_extension("blob.tmp");
            {
                let mut f = fs::File::create(&tmp)?;
                f.write_all(data)?;
                f.sync_all()?;
            }
            fs::rename(&tmp, &path)?;
        }
        Ok(BlobRef {
            hash: hash_hex,
            size: data.len() as u64,
            path,
            mime_type: mime_type.into(),
        })
    }

    /// Read the bytes for `hash`. Returns [`BlobError::NotFound`] if
    /// the blob isn't on disk, [`BlobError::HashMismatch`] if the
    /// on-disk bytes don't hash to `hash` (corruption / tampering).
    pub fn load(&self, hash: &str) -> Result<Vec<u8>, BlobError> {
        let path = self.path_for(hash);
        if !path.exists() {
            return Err(BlobError::NotFound(hash.to_string()));
        }
        let mut buf = Vec::new();
        fs::File::open(&path)?.read_to_end(&mut buf)?;
        let actual = blake3::hash(&buf).to_hex().to_string();
        if actual != hash {
            return Err(BlobError::HashMismatch {
                expected: hash.to_string(),
                actual,
            });
        }
        Ok(buf)
    }

    /// `true` when a blob with this hash is on disk.
    #[must_use]
    pub fn exists(&self, hash: &str) -> bool {
        self.path_for(hash).exists()
    }

    /// Delete the blob for `hash`. Returns
    /// [`BlobError::NotFound`] if the file is already missing.
    pub fn delete(&self, hash: &str) -> Result<(), BlobError> {
        let path = self.path_for(hash);
        if !path.exists() {
            return Err(BlobError::NotFound(hash.to_string()));
        }
        fs::remove_file(&path)?;
        Ok(())
    }

    /// Sum of all blob file sizes (in bytes). Walks the tree on every
    /// call; callers wanting a stable accounting should cache.
    pub fn total_size(&self) -> io::Result<u64> {
        let mut total: u64 = 0;
        for shard in fs::read_dir(&self.base_dir)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path())? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    total = total.saturating_add(entry.metadata()?.len());
                }
            }
        }
        Ok(total)
    }

    /// Compute the on-disk path for a given hash hex string.
    #[must_use]
    pub fn path_for(&self, hash: &str) -> PathBuf {
        let shard = hash.get(..2).unwrap_or("__");
        self.base_dir.join(shard).join(format!("{hash}.blob"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> (tempfile::TempDir, BlobStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::new(dir.path().join("blobs")).expect("new");
        (dir, store)
    }

    #[test]
    fn store_and_load_round_trip() {
        let (_dir, store) = fresh();
        let bytes = b"hello world".to_vec();
        let r = store.store(&bytes, "text/plain").expect("store");
        assert_eq!(r.size, bytes.len() as u64);
        assert_eq!(r.mime_type, "text/plain");
        assert!(store.exists(&r.hash));
        let loaded = store.load(&r.hash).expect("load");
        assert_eq!(loaded, bytes);
    }

    #[test]
    fn dedup_same_content_same_hash() {
        let (_dir, store) = fresh();
        let r1 = store.store(b"hi", "text/plain").expect("a");
        let r2 = store.store(b"hi", "text/plain").expect("b");
        assert_eq!(r1.hash, r2.hash);
        assert_eq!(r1.path, r2.path);
        assert_eq!(store.total_size().expect("size"), 2);
    }

    #[test]
    fn different_content_different_hash() {
        let (_dir, store) = fresh();
        let r1 = store.store(b"hi", "text/plain").expect("a");
        let r2 = store.store(b"hello", "text/plain").expect("b");
        assert_ne!(r1.hash, r2.hash);
    }

    #[test]
    fn load_missing_returns_not_found() {
        let (_dir, store) = fresh();
        let err = store.load(&"0".repeat(64)).expect_err("must err");
        assert!(matches!(err, BlobError::NotFound(_)));
    }

    #[test]
    fn delete_then_missing() {
        let (_dir, store) = fresh();
        let r = store.store(b"to be deleted", "text/plain").expect("store");
        store.delete(&r.hash).expect("delete");
        assert!(!store.exists(&r.hash));
        let err = store.delete(&r.hash).expect_err("err");
        assert!(matches!(err, BlobError::NotFound(_)));
    }

    #[test]
    fn corruption_detected() {
        let (_dir, store) = fresh();
        let r = store.store(b"original", "text/plain").expect("store");
        // Tamper.
        fs::write(&r.path, b"corrupted").expect("write");
        let err = store.load(&r.hash).expect_err("must err");
        assert!(matches!(err, BlobError::HashMismatch { .. }));
    }

    #[test]
    fn total_size_sums_blobs() {
        let (_dir, store) = fresh();
        store.store(b"aaa", "text/plain").expect("a");
        store.store(b"bbbb", "text/plain").expect("b");
        store.store(b"ccccc", "text/plain").expect("c");
        assert_eq!(store.total_size().expect("size"), 3 + 4 + 5);
    }
}
