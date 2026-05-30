//! Phase 8 Task 26 — project encryption bridge surface.
//!
//! Wraps `ProjectStore::{enable_encryption, change_passphrase,
//! export_plaintext_recovery, is_encrypted}` so the renderer's
//! `EncryptionPanel` can drive the SQLCipher workflow without ever
//! seeing raw key material — the passphrase is the only secret
//! crossing IPC.
//!
//! All entry points go through `document::with_workspace[_mut]` for
//! lock discipline.

use std::path::PathBuf;

use kcreate_storage::crypto;

use crate::document::{with_workspace, with_workspace_mut, DocumentBridgeError, Result};

/// Public snapshot returned to the renderer. Mirrors the manifest's
/// `encryption` section: when `enabled` is `false`, the rest of the
/// fields are stub values (empty salt, default iteration count) and
/// the renderer should hide the change-passphrase / export-recovery
/// controls.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionStatus {
    /// Whether the active project's database is SQLCipher-encrypted.
    pub enabled: bool,
    /// PBKDF2 iteration count in use. Reported even when disabled
    /// so the UI can pre-populate cost-factor sliders for the
    /// enable flow.
    pub iterations: u32,
    /// Base64 url-safe (no padding) per-project salt. Empty when
    /// `enabled` is `false`. Surfacing this lets the renderer
    /// display a fingerprint of the active key derivation params.
    pub salt: String,
}

/// Result of [`passphrase_strength`]. The renderer maps the score
/// into a 5-bar meter (weak / fair / good / strong / very strong).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassphraseStrength {
    /// `[0, 4]` matching the standard meter scale.
    pub score: u8,
}

/// Snapshot the current project's encryption status.
pub fn encryption_status() -> Result<EncryptionStatus> {
    with_workspace(|ws| {
        let store = ws.store.lock();
        if let Some(meta) = store.manifest().encryption.as_ref() {
            Ok(EncryptionStatus {
                enabled: meta.enabled,
                iterations: meta.iterations,
                salt: meta.salt.clone(),
            })
        } else {
            Ok(EncryptionStatus {
                enabled: false,
                iterations: crypto::DEFAULT_PBKDF2_ITERATIONS,
                salt: String::new(),
            })
        }
    })
}

/// Score a passphrase using
/// [`kcreate_storage::crypto::passphrase_strength`]. Pure function
/// — does not touch the workspace.
#[must_use]
pub fn passphrase_strength(passphrase: &str) -> PassphraseStrength {
    PassphraseStrength {
        score: crypto::passphrase_strength(passphrase),
    }
}

/// Encrypt a previously-plaintext project. Generates a fresh salt
/// and PBKDF2-derives the SQLCipher key from the supplied
/// passphrase. After this call the on-disk database is ciphertext
/// and the manifest carries the encryption metadata.
pub fn enable_encryption(passphrase: &str) -> Result<EncryptionStatus> {
    with_workspace_mut(|ws| {
        ws.store.lock()
            .enable_encryption(passphrase)
            .map_err(map_store_err)?;
        let store = ws.store.lock();
        let meta = store.manifest().encryption.as_ref().ok_or_else(|| {
            DocumentBridgeError::Internal(
                "enable_encryption succeeded but manifest is missing metadata".to_string(),
            )
        })?;
        Ok(EncryptionStatus {
            enabled: meta.enabled,
            iterations: meta.iterations,
            salt: meta.salt.clone(),
        })
    })
}

/// Rotate the project passphrase. The per-project salt and PBKDF2
/// iteration count are unchanged; only the SQLCipher header key is
/// rewritten. After a successful rotation **only the new passphrase
/// decrypts the database** — the old passphrase no longer works.
/// Both keys are derived against the current salt purely so the
/// rekey can hand SQLCipher the correct "unlock" key.
pub fn change_passphrase(old_passphrase: &str, new_passphrase: &str) -> Result<()> {
    with_workspace_mut(|ws| {
        ws.store.lock()
            .change_passphrase(old_passphrase, new_passphrase)
            .map_err(map_store_err)
    })
}

/// Export a plaintext copy of the project's database to
/// `output_path`. The encrypted source is left untouched; used by
/// the "create recovery backup" flow.
///
/// Requires the mutable workspace helper because the underlying
/// `ProjectStore::export_plaintext_recovery` now closes and re-opens
/// the live SQLCipher connection across the export to avoid
/// Windows-side WAL contention from a second concurrent reader.
pub fn export_plaintext_recovery(passphrase: &str, output_path: PathBuf) -> Result<PathBuf> {
    with_workspace_mut(|ws| {
        ws.store.lock()
            .export_plaintext_recovery(passphrase, &output_path)
            .map_err(map_store_err)
    })
}

fn map_store_err(err: kcreate_storage::ProjectStoreError) -> DocumentBridgeError {
    use kcreate_storage::ProjectStoreError as E;
    match err {
        E::PassphraseRequired
        | E::PassphraseEmpty
        | E::AlreadyEncrypted
        | E::NotEncrypted
        | E::InvalidEncryptionMetadata(_) => {
            DocumentBridgeError::Internal(format!("encryption: {err}"))
        }
        other => DocumentBridgeError::Internal(format!("project store: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serial_test::serial;
    use tempfile::TempDir;

    use super::*;
    use crate::document;

    fn fresh_project() -> (TempDir, PathBuf) {
        document::reset_for_tests();
        let tmp = TempDir::new().expect("tempdir");
        // `project_create` builds `<dir>/<name>.kstudio` itself, so
        // we point it at the tempdir root and let it create the
        // suffixed subdirectory.
        let info = document::project_create("encryption-test", tmp.path()).expect("project_create");
        assert_eq!(info.name, "encryption-test");
        let project_path = tmp.path().join("encryption-test.kstudio");
        // Quiet unused-import warning when sqlcipher path is gated.
        let _ = fs::metadata(&project_path);
        (tmp, project_path)
    }

    #[test]
    #[serial]
    fn status_reports_disabled_for_fresh_project() {
        let (_tmp, _path) = fresh_project();
        let status = encryption_status().expect("status");
        assert!(!status.enabled);
        assert_eq!(status.iterations, crypto::DEFAULT_PBKDF2_ITERATIONS);
        assert!(status.salt.is_empty());
    }

    /// `passphrase_strength` is a pure function — it delegates to
    /// `kcreate_storage::crypto::passphrase_strength` and never
    /// touches the bridge workspace. So this test deliberately does
    /// NOT call `document::reset_for_tests()` (which would suggest a
    /// workspace dependency that isn't there) and is NOT marked
    /// `#[serial]` (no shared mutable state). If a future refactor
    /// makes scoring workspace-dependent, both attributes must come
    /// back together — they're a matched pair, not optional.
    #[test]
    fn strength_meter_returns_score() {
        let weak = passphrase_strength("abc");
        let strong = passphrase_strength("Abc123!@#defghijkl");
        assert_eq!(weak.score, 0);
        assert_eq!(strong.score, 4);
    }

    /// End-to-end happy path: enable encryption, observe the
    /// status flips, rotate the passphrase, export a plaintext
    /// recovery copy. Requires the workspace `rusqlite`
    /// `bundled-sqlcipher-vendored-openssl` feature (default in
    /// this workspace).
    #[test]
    #[serial]
    fn enable_change_export_round_trip() {
        let (_tmp, project_path) = fresh_project();
        let initial = enable_encryption("hunter22-the-strong").expect("enable");
        assert!(initial.enabled);
        assert!(!initial.salt.is_empty());
        assert_eq!(initial.iterations, crypto::DEFAULT_PBKDF2_ITERATIONS);

        let snapshot = encryption_status().expect("status");
        assert!(snapshot.enabled);
        assert_eq!(snapshot.salt, initial.salt);

        change_passphrase("hunter22-the-strong", "newer-hunter22-stronger").expect("rotate");

        let export_path = project_path.join("recovery.sqlite");
        let written = export_plaintext_recovery("newer-hunter22-stronger", export_path.clone())
            .expect("export");
        assert_eq!(written, export_path);
        assert!(written.exists(), "plaintext recovery file should exist");
    }

    /// Re-enabling on an already-encrypted project must surface
    /// the `AlreadyEncrypted` storage error, not silently succeed.
    #[test]
    #[serial]
    fn enable_twice_errors() {
        let (_tmp, _path) = fresh_project();
        enable_encryption("first-pass-strong").expect("enable");
        let second = enable_encryption("second-pass-strong");
        assert!(second.is_err(), "second enable must reject");
    }
}
