// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_store::Store;

use super::backend_err::backend_err;
use super::cache::cache;
use super::cache_key::cache_key;
use super::check_names::check_names;
use super::consts::META_ROOT_KEY_WRAPPED;
use super::rewrapped::Rewrapped;
use super::root_key_state::{root_key_state, RootKeyState};
use super::unwrap_uncached::unwrap_uncached;
use super::wrapped_root_key::WrappedRootKey;
use crate::error::Result;
use crate::vault::{ciphertext_version, VaultTransit};

/// Re-encrypt the stored wrapped key under the Transit key's latest
/// version (after a rotation). Vault's rewrap endpoint does the work, so
/// the key is not re-sent; the new ciphertext must unwrap to the same bytes
/// as the old one before it replaces it. Leaves the store alone when the
/// ciphertext is already on the latest version.
pub fn rewrap(store: &Store, v: &VaultTransit) -> Result<Rewrapped> {
    let w = match root_key_state(store)? {
        RootKeyState::Wrapped(w) => w,
        RootKeyState::Both(_) => {
            return Err(backend_err(
                "this store holds both a software and a wrapped root key; finish with \
                 `pistack keep migrate-to-vault` or `pistack keep migrate-to-software` first",
            ))
        }
        RootKeyState::Software | RootKeyState::Empty => {
            return Err(backend_err(
                "the root key in this store is not wrapped by Vault Transit; nothing to rewrap",
            ))
        }
    };
    check_names(v, &w)?;
    let before = unwrap_uncached(v, &w)?;
    let ciphertext = v.rewrap(&w.ciphertext)?;
    let from_version = w.key_version();
    let to_version = ciphertext_version(&ciphertext);
    if to_version.is_none() || to_version < from_version {
        return Err(backend_err(format!(
            "vault transit rewrap returned version {to_version:?}, older than the stored \
             {from_version:?}; nothing written"
        )));
    }
    let next = WrappedRootKey {
        ciphertext,
        rewrapped_at: Some(Utc::now()),
        ..w.clone()
    };
    let after = unwrap_uncached(v, &next)?;
    if after.bytes() != before.bytes() {
        return Err(backend_err(
            "vault transit rewrap produced a ciphertext that unwraps to a different key; \
             nothing written",
        ));
    }
    if to_version == from_version {
        return Ok(Rewrapped {
            from_version,
            to_version,
            changed: false,
        });
    }
    store.put_meta(META_ROOT_KEY_WRAPPED, &next)?;
    cache().lock().insert(cache_key(v, &next), after);
    tracing::info!(
        from = from_version.unwrap_or(0),
        to = to_version.unwrap_or(0),
        "keep: rewrapped the root key under the latest Transit key version"
    );
    Ok(Rewrapped {
        from_version,
        to_version,
        changed: true,
    })
}
