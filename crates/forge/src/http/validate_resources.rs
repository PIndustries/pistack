// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

pub(crate) fn validate_resources(r: &pistack_types::Resources) -> ApiResult<()> {
    if r.vcpus == 0 || r.memory_mib == 0 {
        return Err(ApiError::bad_request("vcpus and memory_mib must be > 0"));
    }
    Ok(())
}
