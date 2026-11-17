// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

pub(crate) fn parse_record_type(name: &str, raw: &str) -> ApiResult<beacon::RecordType> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "A" => Ok(beacon::RecordType::A),
        "AAAA" => Ok(beacon::RecordType::Aaaa),
        "CNAME" => Ok(beacon::RecordType::Cname),
        "MX" => Ok(beacon::RecordType::Mx),
        "TXT" => Ok(beacon::RecordType::Txt),
        "SRV" => Ok(beacon::RecordType::Srv),
        "NS" => Ok(beacon::RecordType::Ns),
        "PTR" => Ok(beacon::RecordType::Ptr),
        "" => Err(ApiError::bad_request(format!(
            "dns {name}: spec.type is required"
        ))),
        other => Err(ApiError::bad_request(format!(
            "dns {name}: unknown type '{other}'"
        ))),
    }
}
