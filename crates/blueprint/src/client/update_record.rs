// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use serde_json::Value;

use super::about::about;
use super::record_spec::record_spec;
use super::zone_matches::zone_matches;

/// TTL, type, owner, and records follow the document. The zone is fixed
/// once the record set exists.
pub(crate) fn update_record(
    beacon: &beacon::Client,
    store: &Store,
    project: Option<&Id>,
    cur: &beacon::RecordSet,
    name: &str,
    raw: &Value,
) -> ApiResult<bool> {
    let zone = store
        .collection::<beacon::Zone>()
        .get(&cur.spec.zone)
        .map_err(|e| about("dns", name)(e.into()))?;
    let zone_ref = raw.get("zone").and_then(Value::as_str).unwrap_or("");
    if !zone_ref.trim().is_empty() && !zone_matches(&zone, zone_ref) {
        return Err(ApiError::bad_request(format!(
            "dns {name}: zone is immutable; it is '{}'",
            zone.meta.name
        )));
    }
    let mut spec = record_spec(beacon, store, project, name, raw)?;
    beacon::validate_recordset(&zone, &mut spec).map_err(about("dns", name))?;
    if spec == cur.spec {
        return Ok(false);
    }
    beacon
        .update_recordset(&cur.meta.id, Some(cur.meta.revision), |r| {
            r.spec = spec;
            Ok(())
        })
        .map_err(about("dns", name))?;
    Ok(true)
}
