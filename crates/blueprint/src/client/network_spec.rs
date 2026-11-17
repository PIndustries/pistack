// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

use super::spec_is_empty::spec_is_empty;

pub(crate) fn network_spec(name: &str, spec: &Value) -> ApiResult<fabric::NetworkSpec> {
    if spec_is_empty(spec) {
        return Ok(fabric::NetworkSpec::default());
    }
    serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("network {name}: {e}")))
}
