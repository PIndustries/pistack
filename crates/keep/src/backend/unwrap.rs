// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::cache::cache;
use super::cache_key::cache_key;
use super::check_names::check_names;
use super::unwrap_uncached::unwrap_uncached;
use super::wrapped_root_key::WrappedRootKey;
use crate::crypto::RootKey;
use crate::error::Result;
use crate::vault::VaultTransit;

pub(in crate::backend) fn unwrap(v: &VaultTransit, w: &WrappedRootKey) -> Result<RootKey> {
    check_names(v, w)?;
    let ck = cache_key(v, w);
    if let Some(k) = cache().lock().get(&ck) {
        return Ok(k.clone());
    }
    let key = unwrap_uncached(v, w)?;
    cache().lock().insert(ck, key.clone());
    Ok(key)
}
