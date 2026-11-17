// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

use super::spec_is_empty::spec_is_empty;

pub(crate) fn pod_spec(name: &str, spec: &Value) -> ApiResult<loom::PodSpec> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "pod {name}: spec.containers is required"
        )));
    }
    let mut spec: loom::PodSpec = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("pod {name}: {e}")))?;
    loom::http::Api::validate(&mut spec, None)
        .map_err(|e| ApiError::bad_request(format!("pod {name}: {}", e.message)))?;
    Ok(spec)
}
