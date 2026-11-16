// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hmac::Mac;

use crate::chunked::checksum_state::hex_encode::hex_encode;
use crate::chunked::checksum_state::hmac_sha256::HmacSha256;

pub(in crate::chunked) fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key");
    mac.update(msg);
    hex_encode(mac.finalize().into_bytes().as_slice())
}
