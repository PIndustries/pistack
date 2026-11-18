// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// Parse `aa:bb:cc:dd:ee:ff` (or `-` separated).
pub fn parse_mac(s: &str) -> ApiResult<[u8; 6]> {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    if parts.len() != 6 {
        return Err(ApiError::bad_request(format!("invalid mac '{s}'")));
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16)
            .map_err(|_| ApiError::bad_request(format!("invalid mac '{s}'")))?;
    }
    if out[0] & 0x01 != 0 {
        return Err(ApiError::bad_request(format!(
            "mac '{s}' is a multicast address"
        )));
    }
    Ok(out)
}
