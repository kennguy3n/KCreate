//! Passphrase-to-key derivation for project encryption.
//!
//! KCreate uses PBKDF2-HMAC-SHA256 with a 16-byte per-project salt
//! and 200_000 iterations (the OWASP 2023 recommendation for
//! sensitive material on consumer hardware). The salt is stored
//! alongside the encrypted database in `manifest.json` so the
//! same passphrase produces the same key across every machine
//! that opens the project.

use std::num::NonZeroU32;

use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;

/// OWASP-recommended iteration count for PBKDF2-HMAC-SHA256 on
/// consumer hardware (2023). A user can increase this in a future
/// migration by re-keying the database.
pub const DEFAULT_PBKDF2_ITERATIONS: u32 = 200_000;

/// Length of the per-project salt in bytes.
pub const SALT_LEN: usize = 16;

/// Length of the derived encryption key in bytes (256 bits).
pub const KEY_LEN: usize = 32;

/// Generate a cryptographically-secure random salt for a fresh
/// project. The caller must persist this salt — losing it makes
/// the project unrecoverable.
#[must_use]
pub fn generate_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    salt
}

/// Derive a 32-byte key from `passphrase` + `salt` using
/// PBKDF2-HMAC-SHA256. Returns the derived key.
///
/// `iterations` must be > 0; supply
/// [`DEFAULT_PBKDF2_ITERATIONS`] unless a project explicitly
/// stores a different value in its manifest.
#[must_use]
pub fn derive_key(passphrase: &str, salt: &[u8], iterations: NonZeroU32) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    // PBKDF2 is single-threaded and bound by iteration count; the
    // call here is the same one used by the Argon2 fallback
    // candidates we evaluated.
    pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, iterations.get(), &mut out);
    out
}

/// Rough passphrase-strength score in `[0, 4]`, matching the
/// commonly-displayed "weak / fair / good / strong / very strong"
/// scale. The scoring intentionally over-rewards length because
/// the academic literature (NIST SP 800-63B) supports it.
///
/// This is NOT a substitute for refusing common passwords — the
/// caller should also reject passphrases that appear in the
/// `rockyou.txt` top-N list, which is out of scope for the
/// storage crate.
#[must_use]
pub fn passphrase_strength(passphrase: &str) -> u8 {
    let len = passphrase.chars().count();
    let has_lower = passphrase.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = passphrase.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = passphrase.chars().any(|c| c.is_ascii_digit());
    let has_symbol = passphrase
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && !c.is_whitespace());
    let mut score = 0u8;
    if len >= 8 {
        score += 1;
    }
    if len >= 12 {
        score += 1;
    }
    if len >= 16 {
        score += 1;
    }
    if [has_lower, has_upper, has_digit, has_symbol]
        .iter()
        .filter(|b| **b)
        .count()
        >= 3
    {
        score += 1;
    }
    score.min(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_key_is_deterministic() {
        let salt = [42u8; SALT_LEN];
        let it = NonZeroU32::new(1000).unwrap();
        let a = derive_key("hello", &salt, it);
        let b = derive_key("hello", &salt, it);
        assert_eq!(a, b);
    }

    #[test]
    fn different_passphrases_produce_different_keys() {
        let salt = [42u8; SALT_LEN];
        let it = NonZeroU32::new(1000).unwrap();
        assert_ne!(derive_key("a", &salt, it), derive_key("b", &salt, it));
    }

    #[test]
    fn different_salts_produce_different_keys() {
        let it = NonZeroU32::new(1000).unwrap();
        let salt_a = [1u8; SALT_LEN];
        let salt_b = [2u8; SALT_LEN];
        assert_ne!(
            derive_key("hello", &salt_a, it),
            derive_key("hello", &salt_b, it)
        );
    }

    #[test]
    fn salt_is_non_zero_after_generation() {
        let salt = generate_salt();
        // 1-in-2^128 probability of an all-zero salt; treat that as
        // never-happens but the test asserts the generator is wired.
        assert_ne!(salt, [0u8; SALT_LEN]);
    }

    #[test]
    fn passphrase_strength_scores() {
        assert_eq!(passphrase_strength(""), 0);
        assert_eq!(passphrase_strength("abc"), 0);
        assert_eq!(passphrase_strength("abcdefgh"), 1);
        assert_eq!(passphrase_strength("abcdefghijkl"), 2);
        assert_eq!(passphrase_strength("abcdefghijklmnop"), 3);
        assert_eq!(passphrase_strength("Abc123!@#defghijkl"), 4);
    }
}
