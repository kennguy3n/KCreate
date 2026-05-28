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

pub mod annotations;
pub mod blobs;
pub mod brand_versions;
pub mod crypto;
pub mod project_io;
pub mod schema;
pub mod thumbnails;

pub use crypto::{
    derive_key, generate_salt, passphrase_strength, DEFAULT_PBKDF2_ITERATIONS, KEY_LEN, SALT_LEN,
};

pub use annotations::{delete_annotation, list_all, list_for_page, set_resolved, upsert_annotation};
pub use blobs::{BlobError, BlobRef, BlobStore};
pub use brand_versions::{
    diff_brand_kit_versions, list_brand_kit_versions, load_brand_kit_version,
    restore_brand_kit_version, save_brand_kit_version, BrandKitDiff, BrandKitVersion,
};
pub use project_io::{ProjectManifest, ProjectStore, ProjectStoreError};
pub use schema::{Database, DatabaseError, MIGRATIONS};
pub use thumbnails::{
    CachedThumbnail, ThumbnailCache, ThumbnailEncoding, ThumbnailEntry, ThumbnailError,
    ThumbnailIndex, COVER_KEY,
};
