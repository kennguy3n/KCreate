//! `kcreate_storage` — local project persistence.
//!
//! This crate owns three responsibilities:
//!
//! - [`schema`] / [`Database`] — `SQLite` schema with idempotent
//!   migrations. Optionally opens an encrypted database (Phase 1+ via
//!   `SQLCipher`); the API is in place today.
//! - [`blobs`] / [`BlobStore`] — content-addressed binary blob store
//!   using BLAKE3 hashing. Layout: `blobs/{xx}/{full-hash}.blob`.
//! - [`project_io`] / [`ProjectStore`] — the `.kstudio/` folder format
//!   tying the database and blob store together with a `manifest.json`.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod blobs;
pub mod project_io;
pub mod schema;
pub mod thumbnails;

pub use blobs::{BlobError, BlobRef, BlobStore};
pub use project_io::{ProjectManifest, ProjectStore, ProjectStoreError};
pub use schema::{Database, DatabaseError, MIGRATIONS};
pub use thumbnails::{
    CachedThumbnail, ThumbnailCache, ThumbnailEncoding, ThumbnailEntry, ThumbnailError,
    ThumbnailIndex, COVER_KEY,
};
