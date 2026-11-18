// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use super::max_also_notify::MAX_ALSO_NOTIFY;
use super::normalize_name::normalize_name;
use super::notify_addr::notify_addr;
use super::zone_kind::ZoneKind;
use super::zone_spec::ZoneSpec;

pub fn validate_zone(spec: &mut ZoneSpec) -> ApiResult<()> {
    spec.name = normalize_name(&spec.name)?;
    if spec.ttl == 0 {
        return Err(ApiError::bad_request("zone ttl must be greater than zero"));
    }
    if spec.email.trim().is_empty() {
        return Err(ApiError::bad_request("zone email must not be empty"));
    }
    spec.email = spec.email.trim().to_owned();
    if let Some((local, domain)) = spec.email.split_once('@') {
        if local.is_empty() {
            return Err(ApiError::bad_request("zone email has no local part"));
        }
        normalize_name(domain)?;
    }
    let mut ns = Vec::with_capacity(spec.nameservers.len());
    for n in &spec.nameservers {
        let n = normalize_name(n)?;
        if !ns.contains(&n) {
            ns.push(n);
        }
    }
    spec.nameservers = ns;
    let mut also = Vec::with_capacity(spec.also_notify.len());
    for a in &spec.also_notify {
        let a = notify_addr(a)
            .map_err(|e| ApiError::bad_request(format!("also_notify {}", e.message)))?;
        if !also.contains(&a) {
            also.push(a);
        }
    }
    if also.len() > MAX_ALSO_NOTIFY {
        return Err(ApiError::bad_request(format!(
            "a zone names at most {MAX_ALSO_NOTIFY} also_notify addresses"
        )));
    }
    spec.also_notify = also;
    if let ZoneKind::Secondary { primaries } = &spec.kind {
        if !spec.also_notify.is_empty() {
            return Err(ApiError::bad_request(
                "only a primary zone takes also_notify",
            ));
        }
        if primaries.is_empty() {
            return Err(ApiError::bad_request(
                "a secondary zone needs at least one primary",
            ));
        }
        for p in primaries {
            notify_addr(p)?;
        }
    }
    Ok(())
}
