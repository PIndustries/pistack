// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};

use super::found::found;
use super::project_allows::project_allows;

pub(crate) fn ensure_zone(
    beacon: &beacon::Client,
    store: &Store,
    project: Option<&Id>,
    dns: &str,
    zone_ref: &str,
) -> ApiResult<beacon::Zone> {
    let zone_ref = zone_ref.trim();
    if zone_ref.is_empty() {
        return Err(ApiError::bad_request(format!(
            "dns {dns}: spec.zone is required"
        )));
    }
    if let Some(id) = Id::parse(kind::ZONE, zone_ref) {
        let zone = store
            .collection::<beacon::Zone>()
            .get(&id)
            .map_err(ApiError::from)?;
        if !project_allows(project, zone.meta.project.as_ref()) {
            return Err(ApiError::bad_request(format!(
                "dns {dns}: zone '{zone_ref}' is not in this project"
            )));
        }
        return Ok(zone);
    }
    if let Some(existing) = found::<beacon::Zone>(store, project, zone_ref)? {
        return Ok(existing);
    }
    let apex = beacon::normalize_name(zone_ref).map_err(|e| {
        ApiError::bad_request(format!("dns {dns}: zone '{zone_ref}': {}", e.message))
    })?;
    let meta_name = apex.trim_end_matches('.').to_owned();
    if meta_name != zone_ref {
        if let Some(existing) = found::<beacon::Zone>(store, project, &meta_name)? {
            return Ok(existing);
        }
    }
    if let Some(existing) = beacon.zone_by_apex(&apex)? {
        if project_allows(project, existing.meta.project.as_ref()) {
            return Ok(existing);
        }
        return Err(ApiError::bad_request(format!(
            "dns {dns}: zone '{zone_ref}' belongs to another project"
        )));
    }
    beacon
        .create_zone(
            Meta::new(kind::ZONE, meta_name, project.cloned()),
            beacon::ZoneSpec {
                name: apex,
                ..beacon::ZoneSpec::default()
            },
        )
        .map_err(|e| ApiError::new(e.code, format!("dns {dns}: {}", e.message)))
}
