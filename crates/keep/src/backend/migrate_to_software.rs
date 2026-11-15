// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use pistack_store::Store;

use super::backend_err::backend_err;
use super::both_keys_error::both_keys_error;
use super::both_keys_match::both_keys_match;
use super::check_names::check_names;
use super::consts::*;
use super::decode_software::decode_software;
use super::migrated::Migrated;
use super::unwrap_uncached::unwrap_uncached;
use super::wrapped_root_key::WrappedRootKey;
use crate::error::Result;
use crate::vault::VaultTransit;

/// Move a Vault-wrapped root key back into the store in the clear. The
/// reverse of [`migrate_to_vault`](super::migrate_to_vault): unwrap, then write [`META_ROOT_KEY`]
/// and delete [`META_ROOT_KEY_WRAPPED`] in one transaction. Afterwards
/// anyone with the store file has the key.
pub fn migrate_to_software(store: &Store, v: &VaultTransit) -> Result<Migrated> {
    let plain = store.get_meta::<String>(META_ROOT_KEY)?;
    let wrapped = store.get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)?;
    match (plain, wrapped) {
        (_, None) => Err(backend_err(
            "the root key in this store is not wrapped by Vault Transit; nothing to migrate",
        )),
        (Some(b64), Some(w)) => {
            check_names(v, &w)?;
            if !both_keys_match(v, &b64, &w)? {
                return Err(both_keys_error(Some(Ok(false))));
            }
            store.delete_meta(META_ROOT_KEY_WRAPPED)?;
            Ok(Migrated {
                mount: w.mount.clone(),
                key: w.key.clone(),
                key_version: w.key_version(),
                resumed: true,
            })
        }
        (None, Some(w)) => {
            check_names(v, &w)?;
            let key = unwrap_uncached(v, &w)?;
            let b64 = B64.encode(key.bytes());
            if decode_software(&b64)?.bytes() != key.bytes() {
                return Err(backend_err(
                    "root key did not survive encoding; nothing written",
                ));
            }
            store.put_and_delete_meta(META_ROOT_KEY, &b64, META_ROOT_KEY_WRAPPED)?;
            tracing::warn!(
                "keep: root key moved out of Vault; it is now in the store in the clear"
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
