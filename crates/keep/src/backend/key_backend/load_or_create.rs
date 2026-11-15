// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Utc;
use pistack_store::Store;

use super::KeyBackend;
use crate::backend::backend_err::backend_err;
use crate::backend::cache::cache;
use crate::backend::cache_key::cache_key;
use crate::backend::consts::{META_ROOT_KEY, META_ROOT_KEY_WRAPPED, WRAPPED_BY_VAULT};
use crate::backend::refuse_if_sealed_data::refuse_if_sealed_data;
use crate::backend::wrapped_root_key::WrappedRootKey;
use crate::crypto::RootKey;
use crate::error::Result;

impl KeyBackend {
    /// Read the root key, creating and persisting one on first start.
    pub fn load_or_create(&self, store: &Store) -> Result<RootKey> {
        if let Some(key) = self.load_existing(store)? {
            return Ok(key);
        }
        refuse_if_sealed_data(store)?;
        let key = RootKey::generate();
        match self {
            KeyBackend::Software => {
                store.put_meta(META_ROOT_KEY, &B64.encode(key.bytes()))?;
                tracing::info!("keep: generated root key (software backend)");
            }
            KeyBackend::VaultTransit(v) => {
                let ciphertext = v.encrypt(key.bytes())?;
                // Prove we can get it back before anything is sealed under it.
                if v.decrypt(&ciphertext)?.as_slice() != key.bytes() {
                    return Err(backend_err(
                        "vault transit returned a different key on unwrap; not storing it",
                    ));
                }
                let w = WrappedRootKey {
                    backend: WRAPPED_BY_VAULT.into(),
                    mount: v.mount.clone(),
                    key: v.key.clone(),
                    ciphertext,
                    created_at: Utc::now(),
                    rewrapped_at: None,
                };
                store.put_meta(META_ROOT_KEY_WRAPPED, &w)?;
                cache().lock().insert(cache_key(v, &w), key.clone());
                tracing::info!(
                    addr = %v.addr,
                    mount = %v.mount,
                    key = %v.key,
                    "keep: generated root key wrapped by Vault Transit"
                );
            }
        }
        Ok(key)
    }
}
