// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::usable::usable;
use crate::overlay::checks::usable_v6::usable_v6;
use crate::overlay::types::plan_error::PlanError;
use std::net::IpAddr;

/// Guest, gateway, floating, fixed, DHCP, DNS, and route next hops.
/// Display of the parsed address is the canonical form (no leading zeros,
/// compressed IPv6).
pub(in crate::overlay) fn canonical_ip(role: &str, raw: &str) -> Result<IpAddr, PlanError> {
    if raw.is_empty()
        || raw.contains('/')
        || raw.contains('%')
        || raw.chars().any(char::is_whitespace)
    {
        return Err(PlanError::Ip(format!(
            "{role} {raw} is not a usable ip address"
        )));
    }
    match raw.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) if usable(ip) => Ok(IpAddr::V4(ip)),
        Ok(IpAddr::V6(ip)) if usable_v6(ip) => Ok(IpAddr::V6(ip)),
        _ => Err(PlanError::Ip(format!(
            "{role} {raw} is not a usable ip address"
        ))),
    }
}
