// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::{HeaderMap, HeaderValue};

use super::delete_marker::DELETE_MARKER;

/// `x-amz-delete-marker: true`: the call met, created or named a delete marker.
pub(crate) fn set_delete_marker(headers: &mut HeaderMap) {
    headers.insert(DELETE_MARKER, HeaderValue::from_static("true"));
}
