// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::http::{header, HeaderMap, HeaderValue};
use pistack_types::ApiResult;

use super::etag_value::etag_value;

pub(crate) fn apply_object_headers(headers: &mut HeaderMap, obj: &Object) -> ApiResult<()> {
    let ctype = HeaderValue::from_str(&obj.spec.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_TYPE, ctype);
    if let Ok(len) = HeaderValue::from_str(&obj.status.size_bytes.to_string()) {
        headers.insert(header::CONTENT_LENGTH, len);
    }
    if let Some(etag) = etag_value(obj) {
        headers.insert(header::ETAG, etag);
    }
    let last_mod = obj.meta.updated_at.format("%a, %d %b %Y %H:%M:%S GMT");
    if let Ok(v) = HeaderValue::from_str(&last_mod.to_string()) {
        headers.insert(header::LAST_MODIFIED, v);
    }
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    Ok(())
}
