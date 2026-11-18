// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::normalize_etag::normalize_etag;
use super::s3_err::s3_err;

pub(crate) fn hex_decode_md5(hex: &str) -> ApiResult<[u8; 16]> {
    let hex = normalize_etag(hex);
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(s3_err("InvalidPart", "ETag is not a 32-char hex MD5"));
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| s3_err("InvalidPart", "ETag is not a 32-char hex MD5"))?;
    }
    Ok(out)
}
