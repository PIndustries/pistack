// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use super::owner_in_zone::owner_in_zone;
use super::record_set_spec::RecordSetSpec;
use super::record_type::RecordType;
use super::validate_rdata::validate_rdata;
use super::zone::Zone;

/// Normalise and check a record set against its zone. Owner is made
/// absolute, RDATA is syntax-checked per type and canonicalised where it is
/// a name. Does not check CNAME exclusivity against other sets; see
/// `Client`.
pub fn validate_recordset(zone: &Zone, spec: &mut RecordSetSpec) -> ApiResult<()> {
    spec.name = owner_in_zone(&zone.spec.name, &spec.name)?;
    if spec.records.is_empty() {
        return Err(ApiError::bad_request(
            "a record set needs at least one record",
        ));
    }
    if spec.ttl == Some(0) {
        return Err(ApiError::bad_request(
            "record ttl must be greater than zero",
        ));
    }
    let apex = spec.name == zone.spec.name;
    match spec.kind {
        RecordType::Cname if apex => {
            return Err(ApiError::bad_request("a CNAME cannot sit at the zone apex"));
        }
        RecordType::Cname if spec.records.len() > 1 => {
            return Err(ApiError::bad_request(
                "a CNAME set holds exactly one record",
            ));
        }
        RecordType::Ns if apex => {
            return Err(ApiError::bad_request(
                "apex NS records come from the zone's nameservers",
            ));
        }
        _ => {}
    }
    let mut out = Vec::with_capacity(spec.records.len());
    for r in &spec.records {
        let canon = validate_rdata(spec.kind, r)?;
        if out.contains(&canon) {
            return Err(ApiError::bad_request(format!("duplicate record {r}")));
        }
        out.push(canon);
    }
    spec.records = out;
    Ok(())
}
