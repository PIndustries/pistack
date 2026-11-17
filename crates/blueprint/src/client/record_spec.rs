// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use serde_json::Value;

use super::ensure_zone::ensure_zone;
use super::parse_record_type::parse_record_type;
use super::record_doc::RecordDoc;
use super::spec_is_empty::spec_is_empty;

pub(crate) fn record_spec(
    beacon: &beacon::Client,
    store: &Store,
    project: Option<&Id>,
    name: &str,
    spec: &Value,
) -> ApiResult<beacon::RecordSetSpec> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "dns {name}: spec.zone, spec.type, and spec.records are required"
        )));
    }
    let doc: RecordDoc = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("dns {name}: {e}")))?;
    if doc.records.is_empty() {
        return Err(ApiError::bad_request(format!(
            "dns {name}: spec.records is required"
        )));
    }
    let kind_ = parse_record_type(name, &doc.kind)?;
    let zone = ensure_zone(beacon, store, project, name, &doc.zone)?;
    let owner = {
        let given = doc.name.trim();
        if given.is_empty() {
            name.to_owned()
        } else {
            given.to_owned()
        }
    };
    Ok(beacon::RecordSetSpec {
        zone: zone.meta.id,
        name: owner,
        kind: kind_,
        ttl: doc.ttl,
        records: doc.records,
    })
}
