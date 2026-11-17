// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

use super::listener_doc_type::ListenerDoc;
use super::spec_is_empty::spec_is_empty;

pub(crate) fn listener_doc(name: &str, spec: &Value) -> ApiResult<ListenerDoc> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "listener {name}: spec.protocol and spec.port are required"
        )));
    }
    serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("listener {name}: {e}")))
}
