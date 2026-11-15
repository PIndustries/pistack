// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::hex_sha256::hex_sha256;
use crate::chunked::checksum_state::hmac_hex::hmac_hex;

/// Trailer signature for `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`.
pub fn trailer_signature(
    signing_key: &[u8],
    amz_date: &str,
    scope: &str,
    prev_signature: &str,
    trailer_header_line: &str,
) -> String {
    let trailer_hash = hex_sha256(trailer_header_line.as_bytes());
    let sts =
        format!("AWS4-HMAC-SHA256-TRAILER\n{amz_date}\n{scope}\n{prev_signature}\n{trailer_hash}");
    hmac_hex(signing_key, sts.as_bytes())
}
