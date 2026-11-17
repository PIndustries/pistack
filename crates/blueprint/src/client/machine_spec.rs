// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

use super::spec_is_empty::spec_is_empty;

pub(crate) fn machine_spec(name: &str, spec: &Value) -> ApiResult<forge::MachineSpec> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "machine {name}: size and image are required"
        )));
    }
    let spec: forge::MachineSpec = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("machine {name}: {e}")))?;
    let size = spec.size.trim().to_owned();
    let image = spec.image.trim().to_owned();
    if size.is_empty() || image.is_empty() {
        return Err(ApiError::bad_request(format!(
            "machine {name}: size and image are required"
        )));
    }
    let mut spec = spec;
    spec.size = size;
    spec.image = image;
    Ok(spec)
}
