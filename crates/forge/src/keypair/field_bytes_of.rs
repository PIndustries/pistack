// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::ecdsa_p256::ECDSA_P256;
use super::ecdsa_p384::ECDSA_P384;

/// Bits of the field of an accepted `ecdsa` curve.
pub(crate) fn field_bytes_of(key_type: &str) -> usize {
    match key_type {
        ECDSA_P256 => 32,
        ECDSA_P384 => 48,
        _ => 66,
    }
}
