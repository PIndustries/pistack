// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Password hashing (argon2id) and API-key secrets (random, SHA-256 at rest).

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::Engine;
use pistack_types::{ApiError, ApiResult};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Prefix on every API-key secret so one can be recognised in a config file
/// or a leaked log.
pub const API_KEY_PREFIX: &str = "pik_";

/// Shortest accepted password. A sign-in against a hash stored before this
/// rule is not measured against it, so existing passwords keep working.
pub const MIN_PASSWORD_CHARS: usize = 12;

/// Whether a password that is about to be set is one to keep. It must not be
/// the name it is set for — that is what a guesser types first — and it must
/// be long enough. Both are said at once when both are wrong.
pub fn check_password(password: &str, name: &str) -> ApiResult<()> {
    let same = password.eq_ignore_ascii_case(name);
    let short = password.chars().count() < MIN_PASSWORD_CHARS;
    if same && short {
        return Err(ApiError::bad_request(format!(
            "password must not be the same as the user name, and must be at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    if same {
        return Err(ApiError::bad_request(
            "password must not be the same as the user name",
        ));
    }
    if short {
        return Err(ApiError::bad_request(format!(
            "password must be at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    Ok(())
}

/// Hash a password with argon2id and a fresh random salt (PHC string).
/// A hash of anything is produced here; whether the value may be set is
/// [`check_password`]'s question, and the caller that knows the name asks it.
pub fn hash_password(password: &str) -> ApiResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::internal(format!("hashing password: {e}")))
}

/// Constant-time verification of `password` against a PHC string.
pub fn verify_password(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// A random password for the bootstrap admin: 24 URL-safe base64 chars.
pub fn random_password() -> String {
    let mut buf = [0u8; 18];
    OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// A new API-key secret and the hash stored for it.
pub fn new_api_key() -> (String, String) {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    let secret = format!(
        "{API_KEY_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
    );
    let hash = hash_api_key(&secret);
    (secret, hash)
}

/// Hex SHA-256 of an API-key secret. Keys are high-entropy so a fast hash
/// is appropriate here, unlike passwords.
pub fn hash_api_key(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ADMIN_NAME;
    use pistack_types::error::ErrorCode;

    const LONG: &str = "correct-horse";

    fn refusal(password: &str, name: &str) -> String {
        check_password(password, name)
            .err()
            .unwrap_or_else(|| panic!("'{password}' for '{name}' was accepted"))
            .message
    }

    #[test]
    fn password_hash_verifies_and_rejects() {
        let h = hash_password(LONG).unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password(LONG, &h));
        assert!(!verify_password("correct-horsf", &h));
        assert!(!verify_password(LONG, "not-a-phc-string"));
    }

    #[test]
    fn a_password_hash_of_what_is_stored_still_verifies() {
        // A hash written before the length rule existed is not re-measured at
        // sign-in, however short the password behind it was.
        let h = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &h));
    }

    #[test]
    fn hashes_are_salted() {
        let a = hash_password(LONG).unwrap();
        let b = hash_password(LONG).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_new_password_is_too_long_to_guess_and_not_the_name() {
        assert!(check_password(LONG, "alice").is_ok());
        assert!(check_password("twelve-char!", "alice").is_ok());
        assert!(check_password("correct horse", "correct horse").is_err());
        assert_eq!(
            refusal("alice-long-enough", "alice-long-enough"),
            "password must not be the same as the user name"
        );
        assert_eq!(
            refusal("ALICE-LONG-ENOUGH", "alice-long-enough"),
            "password must not be the same as the user name"
        );
        // Both rules broken says both.
        assert_eq!(
            refusal("alice", "alice"),
            "password must not be the same as the user name, and must be at least 12 characters"
        );
        assert_eq!(
            refusal("short one", "alice"),
            "password must be at least 12 characters"
        );
        assert_eq!(
            refusal("", "alice"),
            "password must be at least 12 characters"
        );
        // Counted in characters, not bytes.
        assert!(check_password("duz纠正 horses", "alice").is_ok());
        assert_eq!(
            refusal("纠正", "alice"),
            "password must be at least 12 characters"
        );
    }

    #[test]
    fn a_refused_password_is_a_bad_request() {
        let e = check_password("short", "alice").unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
    }

    #[test]
    fn a_generated_password_passes_the_rule() {
        for _ in 0..20 {
            let p = random_password();
            assert!(check_password(&p, ADMIN_NAME).is_ok(), "{p}");
        }
    }

    #[test]
    fn api_key_roundtrip() {
        let (secret, hash) = new_api_key();
        assert!(secret.starts_with(API_KEY_PREFIX));
        assert_eq!(hash.len(), 64);
        assert_eq!(hash_api_key(&secret), hash);
        let (other, _) = new_api_key();
        assert_ne!(secret, other);
    }
}
