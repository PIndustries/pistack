// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::check_payload_sha256::check_payload_sha256;
use super::expected_payload_sha256::ExpectedPayloadSha256;
use super::hex_sha256::hex_sha256;

/// Hash `body` and compare to a declared SigV4 payload hash (no-op if none).
pub fn verify_body_sha256(expected: Option<&ExpectedPayloadSha256>, body: &[u8]) -> ApiResult<()> {
    check_payload_sha256(expected, &hex_sha256(body))
}
