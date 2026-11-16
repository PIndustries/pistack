// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_store::Store;

use super::backend_err::backend_err;
use super::both_keys_error::both_keys_error;
use super::both_keys_match::both_keys_match;
use super::cache::cache;
use super::cache_key::cache_key;
use super::check_names::check_names;
use super::consts::*;
use super::decode_software::decode_software;
use super::migrated::Migrated;
use super::wrapped_root_key::WrappedRootKey;
use crate::error::Result;
use crate::vault::VaultTransit;

/// Move this store's software root key into Vault Transit.
///
/// Wraps [`META_ROOT_KEY`] with `v`, asks Vault to unwrap the result and
/// compares the bytes, and only then writes [`META_ROOT_KEY_WRAPPED`] and
/// deletes [`META_ROOT_KEY`] in one transaction. Any failure before the
/// commit leaves the store as it was. If the store already holds both keys
/// and they are the same key, the plaintext copy is deleted (finishing an
/// interrupted run); two different keys are refused.
pub fn migrate_to_vault(store: &Store, v: &VaultTransit) -> Result<Migrated> {
    let plain = store.get_meta::<String>(META_ROOT_KEY)?;
    let wrapped = store.get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)?;
    match (plain, wrapped) {
        (None, None) => Err(backend_err(format!(
            "this store has no root key, so there is nothing to migrate; a server started \
             with {ENV_BACKEND}=vault creates a wrapped key on first start"
        ))),
        (None, Some(w)) => Err(backend_err(format!(
            "the root key is already wrapped by Vault Transit (mount '{}', key '{}', v{}); \
             nothing to migrate",
            w.mount,
            w.key,
            w.key_version().unwrap_or(0)
        ))),
        (Some(b64), Some(w)) => {
            check_names(v, &w)?;
            if !both_keys_match(v, &b64, &w)? {
                return Err(both_keys_error(Some(Ok(false))));
            }
            store.delete_meta(META_ROOT_KEY)?;
            tracing::info!(
                "keep: removed the plaintext copy of a root key already wrapped by Vault"
            );
            Ok(Migrated {
                mount: w.mount.clone(),
                key: w.key.clone(),
                key_version: w.key_version(),
                resumed: true,
            })
        }
        (Some(b64), None) => {
            let key = decode_software(&b64)?;
            let ciphertext = v.encrypt(key.bytes())?;
            if v.decrypt(&ciphertext)?.as_slice() != key.bytes() {
                return Err(backend_err(
                    "vault transit unwrapped the new ciphertext to different bytes; \
                     the software key is left in place and nothing was written",
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
            store.put_and_delete_meta(META_ROOT_KEY_WRAPPED, &w, META_ROOT_KEY)?;
            cache().lock().insert(cache_key(v, &w), key);
            tracing::info!(
                mount = %w.mount,
                key = %w.key,
                version = w.key_version().unwrap_or(0),
                "keep: moved the root key into Vault Transit"
            );
            Ok(Migrated {
                mount: w.mount.clone(),
                key: w.key.clone(),
                key_version: w.key_version(),
                resumed: false,
            })
        }
    }
}
