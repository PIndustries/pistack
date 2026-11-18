// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::http::HeaderValue;

pub(crate) fn etag_value(obj: &Object) -> Option<HeaderValue> {
    let hex = if !obj.status.md5.is_empty() {
        &obj.status.md5
    } else if !obj.status.sha256.is_empty() {
        &obj.status.sha256
    } else {
        return None;
    };
    HeaderValue::from_str(&format!("\"{hex}\"")).ok()
}
