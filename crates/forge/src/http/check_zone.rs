// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{MachineSpec, ZONE_LABEL};
use pistack_types::{ApiError, ApiResult};

/// A zone and a `zone` host selector say the same thing. Two different
/// values can match no host, so refuse them here.
pub(crate) fn check_zone(spec: &MachineSpec) -> ApiResult<()> {
    let Some(zone) = &spec.zone else {
        return Ok(());
    };
    if zone.trim().is_empty() {
        return Err(ApiError::bad_request("spec.zone must not be empty"));
    }
    match spec.host_selector.get(ZONE_LABEL) {
        Some(sel) if sel != zone => Err(ApiError::unprocessable(format!(
            "spec.zone is '{zone}' but spec.host_selector asks for zone '{sel}'"
        ))),
        _ => Ok(()),
    }
}
