// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::{HeaderMap, HeaderValue};

use super::version_id::VERSION_ID;

/// `x-amz-version-id`: the record's generation number as a decimal string.
pub(crate) fn set_version_id(headers: &mut HeaderMap, version: u64) {
    if let Ok(v) = HeaderValue::from_str(&version.to_string()) {
        headers.insert(VERSION_ID, v);
    }
}
