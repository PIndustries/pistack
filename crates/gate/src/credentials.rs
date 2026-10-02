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

/// Hash a password with argon2id and a fresh random salt (PHC string).
pub fn hash_password(password: &str) -> ApiResult<String> {
    if password.is_empty() {
        return Err(ApiError::bad_request("password must not be empty"));
    }
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

    #[test]
    fn password_hash_verifies_and_rejects() {
        let h = hash_password("hunter22").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("hunter22", &h));
        assert!(!verify_password("hunter23", &h));
        assert!(!verify_password("hunter22", "not-a-phc-string"));
        assert!(hash_password("").is_err());
    }

    #[test]
    fn hashes_are_salted() {
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b);
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
