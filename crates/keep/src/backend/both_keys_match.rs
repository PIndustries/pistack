// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::check_names::check_names;
use super::decode_software::decode_software;
use super::unwrap_uncached::unwrap_uncached;
use super::wrapped_root_key::WrappedRootKey;
use crate::error::Result;
use crate::vault::VaultTransit;

/// Do the plaintext and the wrapped copy hold the same key?
pub(in crate::backend) fn both_keys_match(
    v: &VaultTransit,
    b64: &str,
    w: &WrappedRootKey,
) -> Result<bool> {
    check_names(v, w)?;
    let plain = decode_software(b64)?;
    let unwrapped = unwrap_uncached(v, w)?;
    Ok(plain.bytes() == unwrapped.bytes())
}
