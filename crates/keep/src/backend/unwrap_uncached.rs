// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::backend_err::backend_err;
use super::too_old_hint::too_old_hint;
use super::wrapped_root_key::WrappedRootKey;
use crate::crypto::RootKey;
use crate::error::Result;
use crate::vault::VaultTransit;

/// Ask Vault, never the cache: the operator commands must prove Vault
/// still unwraps the key.
pub(in crate::backend) fn unwrap_uncached(v: &VaultTransit, w: &WrappedRootKey) -> Result<RootKey> {
    let raw = v.decrypt(&w.ciphertext).map_err(|e| too_old_hint(e, w))?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| backend_err("vault transit unwrapped a root key that is not 32 bytes"))?;
    Ok(RootKey::from_bytes(bytes))
}
