// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use pistack_types::ApiResult;

use super::ct_eq_hex::ct_eq_hex;
use super::expected_payload_sha256::ExpectedPayloadSha256;

/// After a streaming PUT, ensure the body SHA-256 matches the signed declaration.
pub fn check_payload_sha256(
    expected: Option<&ExpectedPayloadSha256>,
    actual_hex: &str,
) -> ApiResult<()> {
    let Some(ExpectedPayloadSha256(want)) = expected else {
        return Ok(());
    };
    if !ct_eq_hex(want, &actual_hex.to_ascii_lowercase()) {
        return Err(s3_err(
            "XAmzContentSHA256Mismatch",
            "The provided 'x-amz-content-sha256' header does not match what was computed.",
        ));
    }
    Ok(())
}
