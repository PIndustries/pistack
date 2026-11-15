// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::hex_sha256::hex_sha256;
use crate::chunked::checksum_state::hmac_hex::hmac_hex;
use crate::sigv4::EMPTY_PAYLOAD_SHA256;

/// AWS chunk signature (documented string-to-sign).
pub fn chunk_signature(
    signing_key: &[u8],
    amz_date: &str,
    scope: &str,
    prev_signature: &str,
    chunk_data: &[u8],
) -> String {
    let chunk_hash = hex_sha256(chunk_data);
    let sts = format!(
        "AWS4-HMAC-SHA256-PAYLOAD\n{amz_date}\n{scope}\n{prev_signature}\n{EMPTY_PAYLOAD_SHA256}\n{chunk_hash}"
    );
    hmac_hex(signing_key, sts.as_bytes())
}
