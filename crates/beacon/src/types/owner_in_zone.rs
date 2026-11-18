// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use super::in_zone::in_zone;
use super::normalize_name::normalize_name;

/// Resolve a record owner relative to the zone and check it falls inside it.
/// `@` and the empty string mean the apex.
pub fn owner_in_zone(zone: &str, raw: &str) -> ApiResult<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "@" {
        return Ok(zone.to_owned());
    }
    let name = if raw.ends_with('.') {
        normalize_name(raw)?
    } else {
        normalize_name(&format!("{raw}.{zone}"))?
    };
    if !in_zone(zone, &name) {
        return Err(ApiError::bad_request(format!(
            "{name} is not inside zone {zone}"
        )));
    }
    Ok(name)
}
