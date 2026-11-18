// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use sha2::{Digest, Sha256};

use super::hex_encode::hex_encode;

pub fn hex_sha256(data: &[u8]) -> String {
    hex_encode(Sha256::digest(data).as_slice())
}
