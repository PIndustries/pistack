// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::ecdsa_p256::ECDSA_P256;
use super::ecdsa_p384::ECDSA_P384;
use super::ecdsa_p521::ECDSA_P521;

/// The `ecdsa` curve an accepted key type names.
pub(crate) fn curve_of(key_type: &str) -> Option<&'static str> {
    match key_type {
        ECDSA_P256 => Some("nistp256"),
        ECDSA_P384 => Some("nistp384"),
        ECDSA_P521 => Some("nistp521"),
        _ => None,
    }
}
