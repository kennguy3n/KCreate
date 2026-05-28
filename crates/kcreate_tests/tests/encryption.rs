//! Phase 8 Block E: SQLCipher encryption at rest.
//!
//! Integration tests for the encrypted SQLite database flow.

use std::num::NonZeroU32;

use kcreate_storage::crypto::{derive_key, generate_salt, passphrase_strength};
use kcreate_storage::Database;

#[test]
fn open_encrypted_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encrypted.db");
    let salt = generate_salt();
    let key = derive_key(
        "correct horse battery staple",
        &salt,
        NonZeroU32::new(10_000).unwrap(),
    );
    let db = Database::open_encrypted(&path, &key).unwrap();
    assert_eq!(db.path(), path.as_path());
    drop(db);
    // Re-open with the same key — should succeed.
    let _db = Database::open_encrypted(&path, &key).unwrap();
}

#[test]
fn wrong_key_fails_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encrypted.db");
    let salt = generate_salt();
    let it = NonZeroU32::new(10_000).unwrap();
    let key = derive_key("right passphrase", &salt, it);
    let db = Database::open_encrypted(&path, &key).unwrap();
    drop(db);
    let bad_key = derive_key("wrong passphrase", &salt, it);
    let err = Database::open_encrypted(&path, &bad_key);
    assert!(
        err.is_err(),
        "opening with the wrong key must fail; got {:?}",
        err.ok().map(|_| ())
    );
}

#[test]
fn change_key_rekeys_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encrypted.db");
    let salt = generate_salt();
    let it = NonZeroU32::new(10_000).unwrap();
    let old_key = derive_key("old passphrase", &salt, it);
    let new_key = derive_key("new passphrase", &salt, it);
    {
        let _db = Database::open_encrypted(&path, &old_key).unwrap();
    }
    Database::change_key(&path, &old_key, &new_key).unwrap();
    // Now only the new key opens it.
    assert!(Database::open_encrypted(&path, &old_key).is_err());
    let _db = Database::open_encrypted(&path, &new_key).unwrap();
}

#[test]
fn passphrase_strength_meter_grades_input() {
    // Empty / very short passphrases score 0.
    assert_eq!(passphrase_strength(""), 0);
    assert!(passphrase_strength("x") <= 1);
    // Long mixed passphrases score the max.
    let strong = "correct horse battery staple 2024!";
    assert!(passphrase_strength(strong) >= 3);
}

#[test]
fn encrypt_existing_plaintext_then_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.db");
    // Open as plaintext, write nothing.
    {
        let _db = Database::open(&path).unwrap();
    }
    let salt = generate_salt();
    let key = derive_key("secret", &salt, NonZeroU32::new(10_000).unwrap());
    let encrypted_path = Database::encrypt_existing(&path, &key).unwrap();
    let _db = Database::open_encrypted(&encrypted_path, &key).unwrap();
}
