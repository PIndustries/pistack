// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Encryption at rest: the software key backend.
//!
//! Every secret payload and every private key `keep` stores is sealed with
//! AES-256-GCM under one root key. The root key is generated on first start
//! and kept in the store's `meta` table under [`META_ROOT_KEY`], which means
//! an attacker with the store file has the key too. This is the *software*
//! backend: it makes the on-disk format and the API right so a PKCS#11 HSM
//! or Vault transit backend can replace [`RootKey`] later without touching
//! the resource model. Do not mistake it for an HSM.
//!
//! The associated data of each seal is the owning resource's id, so a blob
//! copied from one secret onto another fails to open.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use pistack_store::Store;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::{KeepError, Result};

/// Store meta key holding the base64 root key.
pub const META_ROOT_KEY: &str = "keep/root_key";
const ALG: &str = "aes-256-gcm";

/// A sealed blob as persisted: algorithm tag, 96-bit nonce and ciphertext
/// (with the 128-bit GCM tag appended), both base64.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sealed {
    pub alg: String,
    pub nonce: String,
    pub ciphertext: String,
}

/// The cloud's data-encryption key. Cheap to clone; the bytes live once.
#[derive(Clone)]
pub struct RootKey {
    bytes: Arc<[u8; 32]>,
}

impl std::fmt::Debug for RootKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RootKey(..)")
    }
}

impl RootKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self {
            bytes: Arc::new(bytes),
        }
    }

    /// Fresh random key.
    pub fn generate() -> Self {
        let mut b = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut b);
        Self::from_bytes(b)
    }

    /// Read the root key when one is already stored. Does not create a key.
    pub fn load_existing(store: &Store) -> Result<Option<Self>> {
        let Some(b64) = store.get_meta::<String>(META_ROOT_KEY)? else {
            return Ok(None);
        };
        let raw = B64
            .decode(b64.trim())
            .map_err(|e| KeepError::Crypto(format!("root key is not base64: {e}")))?;
        let bytes: [u8; 32] = raw
            .try_into()
            .map_err(|_| KeepError::Crypto("root key must be 32 bytes".into()))?;
        Ok(Some(Self::from_bytes(bytes)))
    }

    /// Read the root key from the store, generating and persisting one on
    /// first start.
    pub fn load_or_create(store: &Store) -> Result<Self> {
        if let Some(key) = Self::load_existing(store)? {
            return Ok(key);
        }
        let key = Self::generate();
        store.put_meta(META_ROOT_KEY, &B64.encode(*key.bytes))?;
        tracing::info!("keep: generated root key (software backend)");
        Ok(key)
    }

    fn cipher(&self) -> Result<Aes256Gcm> {
        Aes256Gcm::new_from_slice(&self.bytes[..])
            .map_err(|e| KeepError::Crypto(format!("bad key length: {e}")))
    }

    /// Encrypt `plaintext`, binding it to `aad` (normally the owner's id).
    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<Sealed> {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = self
            .cipher()?
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| KeepError::Crypto("encryption failed".into()))?;
        Ok(Sealed {
            alg: ALG.to_owned(),
            nonce: B64.encode(nonce),
            ciphertext: B64.encode(ct),
        })
    }

    /// Decrypt a blob sealed with [`seal`](Self::seal) under the same `aad`.
    /// Any change to nonce, ciphertext, tag or aad fails.
    pub fn open(&self, sealed: &Sealed, aad: &[u8]) -> Result<Vec<u8>> {
        if sealed.alg != ALG {
            return Err(KeepError::Crypto(format!(
                "unsupported algorithm {}",
                sealed.alg
            )));
        }
        let nonce = B64
            .decode(&sealed.nonce)
            .map_err(|e| KeepError::Crypto(format!("nonce: {e}")))?;
        if nonce.len() != 12 {
            return Err(KeepError::Crypto("nonce must be 12 bytes".into()));
        }
        let ct = B64
            .decode(&sealed.ciphertext)
            .map_err(|e| KeepError::Crypto(format!("ciphertext: {e}")))?;
        self.cipher()?
            .decrypt(Nonce::from_slice(&nonce), Payload { msg: &ct, aad })
            .map_err(|_| KeepError::Crypto("decryption failed: wrong key or tampered data".into()))
    }
}

/// Hex SHA-256 of `data`; used for join-token hashes and fingerprints.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(data);
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 32 random bytes, URL-safe base64 without padding (43 chars).
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip_and_tamper_detection() {
        let key = RootKey::generate();
        let sealed = key.seal(b"hunter2", b"sec-1").unwrap();
        assert_eq!(key.open(&sealed, b"sec-1").unwrap(), b"hunter2");

        // Different aad: fails.
        assert!(key.open(&sealed, b"sec-2").is_err());

        // Flip a ciphertext byte: fails.
        let mut ct = B64.decode(&sealed.ciphertext).unwrap();
        ct[0] ^= 0x01;
        let tampered = Sealed {
            ciphertext: B64.encode(ct),
            ..sealed.clone()
        };
        assert!(key.open(&tampered, b"sec-1").is_err());

        // Flip a tag byte: fails.
        let mut ct = B64.decode(&sealed.ciphertext).unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0x80;
        let tampered = Sealed {
            ciphertext: B64.encode(ct),
            ..sealed.clone()
        };
        assert!(key.open(&tampered, b"sec-1").is_err());

        // Different key: fails.
        assert!(RootKey::generate().open(&sealed, b"sec-1").is_err());

        // Nonces are unique per seal.
        let again = key.seal(b"hunter2", b"sec-1").unwrap();
        assert_ne!(again.nonce, sealed.nonce);
    }

    #[test]
    fn root_key_persists_in_store() {
        let store = Store::in_memory().unwrap();
        let a = RootKey::load_or_create(&store).unwrap();
        let b = RootKey::load_or_create(&store).unwrap();
        assert_eq!(*a.bytes, *b.bytes);
        assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_some());
    }

    #[test]
    fn hashes_and_tokens() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let t = random_token();
        assert_eq!(t.len(), 43);
        assert_ne!(t, random_token());
    }
}
