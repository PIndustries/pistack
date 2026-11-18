// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use pistack_types::{ApiError, ApiResult};

/// Decode an opaque continuation token. Errors must not echo raw bytes.
pub(crate) fn decode_continuation_token(raw: &str) -> ApiResult<String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .map_err(|_| ApiError::bad_request("continuation-token is invalid"))?;
    String::from_utf8(bytes).map_err(|_| ApiError::bad_request("continuation-token is invalid"))
}
