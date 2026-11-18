// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

pub(crate) fn require_primary(zone: &Zone) -> ApiResult<()> {
    if matches!(zone.spec.kind, ZoneKind::Primary) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "record sets belong on primary zones; secondaries are transfer-only",
        ))
    }
}
