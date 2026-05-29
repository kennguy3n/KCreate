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
pub mod guides;
pub mod project_io;
pub mod schema;
pub mod thumbnails;

pub use crypto::{
    derive_key, generate_salt, passphrase_strength, DEFAULT_PBKDF2_ITERATIONS, KEY_LEN, SALT_LEN,
};

pub use annotations::{
    delete_annotation, list_all, list_for_page, set_resolved, upsert_annotation,
};
pub use guides::{
    delete_all_for_page as delete_all_guides_for_page, delete_guide,
    list_all as list_all_guides, list_for_page as list_guides_for_page,
    load_grid_settings, upsert_grid_settings, upsert_guide, Guide, GridSettings,
    GuideOrientation,
};
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
