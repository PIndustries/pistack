// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};

/// Zones resolve by resource name, id, or apex (`example.cloud.`).
pub(crate) fn resolve_zone(c: &Client, project: Option<&Id>, id: &str) -> ApiResult<Zone> {
    match c.zones().resolve(project, id) {
        Ok(z) => Ok(z),
        Err(e) if id.contains('.') => c.zone_by_apex(id)?.ok_or_else(|| ApiError::from(e)),
        Err(e) => Err(e.into()),
    }
}
