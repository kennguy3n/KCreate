//! Per-project thumbnail cache (`<project>/.kstudio/thumbnails/`).
//!
//! The cache is **lazy**: thumbnails are only materialised when a
//! caller asks for one, and only re-rendered when the underlying
//! content hash changes. The bridge layer
//! (`crates/kcreate_bridge/src/thumbnails.rs`) is the only writer in
//! the codebase — it computes a content hash for the addressed page
//! plus a render of the scene, hands the bytes to [`ThumbnailCache`],
//! and trusts the cache to dedup, persist, and prune.
//!
//! Layout on disk (inside the `.kstudio/thumbnails/` directory):
//!
//! ```text
//! thumbnails/
//!   index.json                              <-- ThumbnailIndex (atomic-replace)
//!   cover.<short-hash>.<ext>                <-- whole-project cover
//!   page-<page-uuid>.<short-hash>.<ext>     <-- per-page thumbs
//! ```
//!
//! Why a separate cover file (instead of "the first page's thumb"):
//! the HomePage's recent-projects list reads the manifest plus the
//! cover *without opening the SQLite database*. Per-page thumbs
//! require knowing which page id is "first" — which means cracking
//! the project. The cover file lets us paint the recent-projects
//! grid for N projects with N small reads (manifest + cover) and
//! zero DB connections.
//!
//! ## Invariants
//!
//! - The index is the source of truth for "what's cached right now".
//!   Files on disk that aren't in the index are treated as orphaned
//!   and reaped by [`ThumbnailCache::compact`].
//! - Writes are atomic: we render to a `*.tmp` file in the same
//!   directory and `rename` into the final path, then re-serialise
//!   the index file the same way. A crash mid-write leaves the
//!   previous (valid) thumbnail in place.
//! - The content hash uniquely identifies a thumbnail — re-saving a
//!   project that hasn't visually changed is a no-op.
//! - The cache never blocks on rendering; it just stores already-
//!   encoded image bytes. All rendering happens in the bridge.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Index filename inside the thumbnails directory.
const INDEX_FILENAME: &str = "index.json";

/// Sentinel page-id used for the project-level cover thumbnail.
///
/// Cover entries live alongside per-page entries in the same index;
/// using a distinct sentinel uuid means the same `lookup` /
/// `store` / `evict` paths work for both. The all-zeroes uuid is
/// reserved for this purpose — no real document node ever has it
/// (real ids come from `Uuid::new_v4`).
pub const COVER_KEY: Uuid = Uuid::nil();

/// Supported encodings for cached thumbnails.
///
/// Both encoders ship with the `image` crate's default feature set.
/// We pick at render-time via the bridge so the cache itself stays
/// content-agnostic (it just stores opaque bytes + an extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThumbnailEncoding {
    Png,
    Webp,
}

impl ThumbnailEncoding {
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

/// One cached thumbnail descriptor (serialised into `index.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThumbnailEntry {
    /// BLAKE3 hex of the content that produced this thumbnail.
    /// Length 64 chars; the cache only stores the first 16 in the
    /// filename to keep paths short while still being effectively
    /// collision-free for a single project.
    pub content_hash: String,
    /// Pixel dimensions of the stored image.
    pub width: u32,
    pub height: u32,
    /// File size in bytes (kept so callers can budget reads without
    /// touching the filesystem).
    pub byte_size: u64,
    /// Filename inside the thumbnails directory (relative, not absolute).
    /// Reconstructed from `(key, hash_prefix, encoding)` but stored
    /// explicitly so the cache survives a renaming-scheme change.
    pub file_name: String,
    /// Encoding of the file (mirrors the extension).
    pub encoding: ThumbnailEncoding,
    /// When this entry was last written.
    pub updated_at: DateTime<Utc>,
}

/// Persistent index of cached thumbnails for a single project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThumbnailIndex {
    /// Schema version of the index file. Currently 1.
    pub version: u32,
    /// `page_id` → entry. Uses `BTreeMap` for deterministic JSON
    /// output so two saves with the same content produce byte-
    /// identical files (helpful for content-addressed storage on
    /// top of the cache — e.g. backups).
    pub entries: BTreeMap<Uuid, ThumbnailEntry>,
}

impl ThumbnailIndex {
    fn new() -> Self {
        Self {
            version: 1,
            entries: BTreeMap::new(),
        }
    }
}

/// Errors from the thumbnail cache.
#[derive(Debug, Error)]
pub enum ThumbnailError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported index version {0}; rebuild required")]
    UnsupportedVersion(u32),
    #[error("empty content hash; refusing to store")]
    EmptyContentHash,
}

/// Lazy thumbnail cache rooted at a `.kstudio/thumbnails/` directory.
#[derive(Debug)]
pub struct ThumbnailCache {
    dir: PathBuf,
    index: ThumbnailIndex,
}

impl ThumbnailCache {
    /// Open (or create) the cache at `dir`. The directory is created
    /// if missing.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, ThumbnailError> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let index_path = dir.join(INDEX_FILENAME);
        let index = if index_path.exists() {
            let raw = fs::read(&index_path)?;
            let parsed: ThumbnailIndex = serde_json::from_slice(&raw).unwrap_or_default();
            if parsed.version > 1 {
                return Err(ThumbnailError::UnsupportedVersion(parsed.version));
            }
            if parsed.version == 0 {
                // Either a freshly-`Default`-constructed file or an
                // older snapshot without a version. Treat as v1.
                ThumbnailIndex {
                    version: 1,
                    entries: parsed.entries,
                }
            } else {
                parsed
            }
        } else {
            ThumbnailIndex::new()
        };
        Ok(Self { dir, index })
    }

    /// Directory holding the cache (the `.kstudio/thumbnails/` dir).
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Read-only view of the index. The caller may iterate but should
    /// not assume entries stay valid after a `store` / `evict` call.
    #[must_use]
    pub const fn index(&self) -> &ThumbnailIndex {
        &self.index
    }

    /// Look up an existing thumbnail. Returns `Some(bytes)` only when
    /// the cached `content_hash` matches `expected_hash` AND the
    /// underlying file is readable. A mismatched hash is treated as
    /// a cache miss (and the next `store` call will overwrite the
    /// stale entry).
    pub fn lookup(
        &self,
        key: Uuid,
        expected_hash: &str,
    ) -> Result<Option<CachedThumbnail>, ThumbnailError> {
        let Some(entry) = self.index.entries.get(&key) else {
            return Ok(None);
        };
        if entry.content_hash != expected_hash {
            return Ok(None);
        }
        let path = self.dir.join(&entry.file_name);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(CachedThumbnail {
                bytes,
                width: entry.width,
                height: entry.height,
                encoding: entry.encoding,
                content_hash: entry.content_hash.clone(),
                updated_at: entry.updated_at,
            })),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Index says "have it" but the file is gone — treat as miss.
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Cheap "do we have a thumb for this key?" check that doesn't
    /// touch the filesystem. Useful for the HomePage's "show
    /// placeholder vs. show thumbnail" decision.
    #[must_use]
    pub fn has(&self, key: Uuid) -> bool {
        self.index.entries.contains_key(&key)
    }

    /// Persist `bytes` as the thumbnail for `key` with the given
    /// content hash. Atomic on Unix and Windows (NTFS): write to a
    /// `*.tmp` file in the same directory, then `rename`. The old
    /// thumbnail file is unlinked after the new index is committed
    /// so a crash never leaves the cache pointing at a missing file.
    pub fn store(
        &mut self,
        key: Uuid,
        content_hash: &str,
        bytes: Vec<u8>,
        width: u32,
        height: u32,
        encoding: ThumbnailEncoding,
    ) -> Result<(), ThumbnailError> {
        if content_hash.is_empty() {
            return Err(ThumbnailError::EmptyContentHash);
        }
        let byte_size = bytes.len() as u64;
        let file_name = thumbnail_filename(key, content_hash, encoding);
        let final_path = self.dir.join(&file_name);
        let tmp_path = self.dir.join(format!("{file_name}.tmp"));

        write_atomic(&tmp_path, &final_path, &bytes)?;

        let entry = ThumbnailEntry {
            content_hash: content_hash.to_string(),
            width,
            height,
            byte_size,
            file_name: file_name.clone(),
            encoding,
            updated_at: Utc::now(),
        };
        let previous = self.index.entries.insert(key, entry);
        self.persist_index()?;

        // Best-effort: unlink the prior file if the filename changed
        // (different content hash → different name). Failure here is
        // not fatal — it just leaves an orphan that `compact` will
        // sweep on a future call.
        if let Some(prev) = previous {
            if prev.file_name != file_name {
                let _ = fs::remove_file(self.dir.join(&prev.file_name));
            }
        }
        Ok(())
    }

    /// Drop the entry for `key` (e.g. when its page was deleted).
    /// Returns whether anything was removed.
    pub fn evict(&mut self, key: Uuid) -> Result<bool, ThumbnailError> {
        if let Some(prev) = self.index.entries.remove(&key) {
            let _ = fs::remove_file(self.dir.join(&prev.file_name));
            self.persist_index()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Drop entries whose keys are not in `live`. Useful when a page
    /// list comes back from the document graph — anything the cache
    /// knows about that isn't in `live` is stale.
    ///
    /// `live` should always include [`COVER_KEY`] if the caller
    /// wants the cover thumbnail to survive; the function does NOT
    /// implicitly preserve it (so callers that genuinely want to
    /// clear everything can do so by passing an empty set).
    pub fn retain_only(&mut self, live: &HashSet<Uuid>) -> Result<usize, ThumbnailError> {
        let mut removed = 0;
        let mut victims: Vec<Uuid> = Vec::new();
        for key in self.index.entries.keys() {
            if !live.contains(key) {
                victims.push(*key);
            }
        }
        for key in victims {
            if let Some(prev) = self.index.entries.remove(&key) {
                let _ = fs::remove_file(self.dir.join(&prev.file_name));
                removed += 1;
            }
        }
        if removed > 0 {
            self.persist_index()?;
        }
        Ok(removed)
    }

    /// Sweep orphaned files in the cache directory (files not
    /// referenced by the current index). Safe to call at any time;
    /// returns the number of files removed.
    pub fn compact(&mut self) -> Result<usize, ThumbnailError> {
        let referenced: HashSet<String> = self
            .index
            .entries
            .values()
            .map(|e| e.file_name.clone())
            .collect();
        let mut removed = 0;
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            let is_tmp = std::path::Path::new(name_str)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"));
            if name_str == INDEX_FILENAME || is_tmp {
                // Leave the index alone; sweep tmp files separately
                // (a tmp file means a write was interrupted).
                if is_tmp {
                    let _ = fs::remove_file(entry.path());
                    removed += 1;
                }
                continue;
            }
            if !referenced.contains(name_str) {
                let _ = fs::remove_file(entry.path());
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Serialise the index to disk via the same atomic-rename dance
    /// used by [`Self::store`].
    fn persist_index(&self) -> Result<(), ThumbnailError> {
        let final_path = self.dir.join(INDEX_FILENAME);
        let tmp_path = self.dir.join(format!("{INDEX_FILENAME}.tmp"));
        let bytes = serde_json::to_vec_pretty(&self.index)?;
        write_atomic(&tmp_path, &final_path, &bytes)?;
        Ok(())
    }
}

/// Bytes + metadata returned from [`ThumbnailCache::lookup`].
#[derive(Debug, Clone)]
pub struct CachedThumbnail {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub encoding: ThumbnailEncoding,
    pub content_hash: String,
    pub updated_at: DateTime<Utc>,
}

/// Build the canonical filename for a (key, hash, encoding) triple.
///
/// We keep the hash prefix in the filename (not the index alone) so
/// users can sanity-check a `.kstudio/thumbnails/` directory by eye
/// after a backup restore — and so that a corrupted index can be
/// rebuilt by walking the directory.
fn thumbnail_filename(key: Uuid, content_hash: &str, encoding: ThumbnailEncoding) -> String {
    let prefix: String = content_hash.chars().take(16).collect();
    let stem = if key == COVER_KEY {
        "cover".to_string()
    } else {
        format!("page-{key}")
    };
    format!("{stem}.{prefix}.{}", encoding.extension())
}

/// Atomic write: stage to `tmp`, fsync, rename onto `final_path`.
fn write_atomic(tmp_path: &Path, final_path: &Path, bytes: &[u8]) -> io::Result<()> {
    {
        let mut f = fs::File::create(tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    match fs::rename(tmp_path, final_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Cleanup the tmp file so we don't leak it.
            let _ = fs::remove_file(tmp_path);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_directory_and_empty_index() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbnailCache::open(dir.path().join("thumbs")).unwrap();
        assert!(cache.dir().exists());
        assert!(cache.index().entries.is_empty());
    }

    #[test]
    fn store_then_lookup_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ThumbnailCache::open(dir.path()).unwrap();
        let key = Uuid::new_v4();
        cache
            .store(
                key,
                "abc123",
                b"PNGDATA".to_vec(),
                16,
                16,
                ThumbnailEncoding::Png,
            )
            .unwrap();
        let cached = cache.lookup(key, "abc123").unwrap().expect("hit");
        assert_eq!(cached.bytes, b"PNGDATA");
        assert_eq!(cached.width, 16);
        assert_eq!(cached.height, 16);
        assert_eq!(cached.encoding, ThumbnailEncoding::Png);
    }

    #[test]
    fn lookup_with_mismatched_hash_is_miss() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ThumbnailCache::open(dir.path()).unwrap();
        let key = Uuid::new_v4();
        cache
            .store(key, "v1", b"old".to_vec(), 8, 8, ThumbnailEncoding::Png)
            .unwrap();
        assert!(cache.lookup(key, "v2").unwrap().is_none());
    }

    #[test]
    fn store_overwrites_replaces_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ThumbnailCache::open(dir.path()).unwrap();
        let key = Uuid::new_v4();
        cache
            .store(key, "v1", b"old".to_vec(), 8, 8, ThumbnailEncoding::Png)
            .unwrap();
        let old_name = cache.index().entries[&key].file_name.clone();
        cache
            .store(key, "v2", b"newdata".to_vec(), 8, 8, ThumbnailEncoding::Png)
            .unwrap();
        let new_name = cache.index().entries[&key].file_name.clone();
        assert_ne!(old_name, new_name);
        // Old file is unlinked.
        assert!(!cache.dir.join(&old_name).exists());
        // New file holds the new bytes.
        assert_eq!(fs::read(cache.dir.join(&new_name)).unwrap(), b"newdata");
    }

    #[test]
    fn evict_removes_entry_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ThumbnailCache::open(dir.path()).unwrap();
        let key = Uuid::new_v4();
        cache
            .store(key, "v1", b"x".to_vec(), 8, 8, ThumbnailEncoding::Png)
            .unwrap();
        let file = cache.dir.join(&cache.index().entries[&key].file_name);
        assert!(file.exists());
        assert!(cache.evict(key).unwrap());
        assert!(!file.exists());
        assert!(!cache.has(key));
    }

    #[test]
    fn retain_only_drops_stale_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ThumbnailCache::open(dir.path()).unwrap();
        let alive = Uuid::new_v4();
        let stale = Uuid::new_v4();
        cache
            .store(alive, "a", b"a".to_vec(), 8, 8, ThumbnailEncoding::Png)
            .unwrap();
        cache
            .store(stale, "b", b"b".to_vec(), 8, 8, ThumbnailEncoding::Png)
            .unwrap();
        let live: HashSet<Uuid> = std::iter::once(alive).collect();
        let removed = cache.retain_only(&live).unwrap();
        assert_eq!(removed, 1);
        assert!(cache.has(alive));
        assert!(!cache.has(stale));
    }

    #[test]
    fn compact_sweeps_orphan_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ThumbnailCache::open(dir.path()).unwrap();
        // Drop a stray file with the right extension.
        let orphan = cache.dir.join("page-deadbeef.aaaa.png");
        fs::write(&orphan, b"junk").unwrap();
        // And a stray tmp file.
        let tmp = cache.dir.join("scratch.tmp");
        fs::write(&tmp, b"x").unwrap();
        let removed = cache.compact().unwrap();
        assert_eq!(removed, 2);
        assert!(!orphan.exists());
        assert!(!tmp.exists());
    }

    #[test]
    fn round_trip_index_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let key = Uuid::new_v4();
        {
            let mut cache = ThumbnailCache::open(dir.path()).unwrap();
            cache
                .store(key, "ham", b"PNG".to_vec(), 32, 24, ThumbnailEncoding::Png)
                .unwrap();
        }
        let cache = ThumbnailCache::open(dir.path()).unwrap();
        let entry = cache.index().entries.get(&key).expect("persisted");
        assert_eq!(entry.content_hash, "ham");
        assert_eq!(entry.width, 32);
        assert_eq!(entry.height, 24);
    }

    #[test]
    fn cover_uses_cover_filename_stem() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ThumbnailCache::open(dir.path()).unwrap();
        cache
            .store(
                COVER_KEY,
                "1234567890abcdef0000",
                b"PNG".to_vec(),
                64,
                64,
                ThumbnailEncoding::Png,
            )
            .unwrap();
        let name = cache.index().entries[&COVER_KEY].file_name.clone();
        assert!(name.starts_with("cover."), "got {name}");
        assert!(std::path::Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png")));
    }

    #[test]
    fn empty_hash_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ThumbnailCache::open(dir.path()).unwrap();
        let err = cache
            .store(
                Uuid::new_v4(),
                "",
                b"x".to_vec(),
                8,
                8,
                ThumbnailEncoding::Png,
            )
            .unwrap_err();
        assert!(matches!(err, ThumbnailError::EmptyContentHash));
    }
}
