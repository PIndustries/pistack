// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::wrapped_root_key::WrappedRootKey;
use crate::crypto::sha256_hex;
use crate::vault::VaultTransit;

pub(in crate::backend) fn cache_key(v: &VaultTransit, w: &WrappedRootKey) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        v.addr,
        v.namespace.as_deref().unwrap_or(""),
        v.mount,
        v.key,
        w.ciphertext,
        sha256_hex(v.token.as_bytes())
    )
}
