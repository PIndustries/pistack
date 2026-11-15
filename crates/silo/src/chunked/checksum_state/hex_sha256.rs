// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::hex_encode::hex_encode;
use sha2::{Digest, Sha256};

pub(in crate::chunked) fn hex_sha256(data: &[u8]) -> String {
    hex_encode(Sha256::digest(data).as_slice())
}
