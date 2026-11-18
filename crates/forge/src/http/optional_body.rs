// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde::de::DeserializeOwned;

/// The JSON body of an action that can also be posted with no body.
pub(crate) fn optional_body<T: DeserializeOwned + Default>(bytes: &[u8]) -> ApiResult<T> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|e| ApiError::bad_request(e.to_string()))
}
