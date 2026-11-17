// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `validate_document`: reject an empty or non-JSON/YAML document.

use pistack_types::{ApiError, ApiResult};

/// Reject an empty document. Otherwise accept JSON (`serde_json::Value`) or
/// text that looks like YAML: it contains `:` and does not start with `{`.
pub fn validate_document(document: &str) -> ApiResult<()> {
    let trimmed = document.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("document must not be empty"));
    }
    if serde_json::from_str::<serde_json::Value>(document).is_ok() {
        return Ok(());
    }
    if trimmed.contains(':') && !trimmed.starts_with('{') {
        return Ok(());
    }
    Err(ApiError::bad_request("document must be JSON or YAML"))
}
