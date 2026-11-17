// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

use super::doc_item_type::DocItem;

/// One value of a parsed stream, as a document item.
pub(crate) fn doc_item(value: Value) -> ApiResult<DocItem> {
    if !value.is_object() {
        return Err(ApiError::bad_request(
            "each document must be a resource mapping",
        ));
    }
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_owned();
    if kind.is_empty() {
        return Err(ApiError::bad_request("resource is missing kind"));
    }
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let spec = value.get("spec").cloned().unwrap_or(Value::Null);
    Ok(DocItem { kind, name, spec })
}
