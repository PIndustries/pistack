// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::ip_err::ip_err;
use crate::overlay::checks::usable::usable;
use crate::overlay::types::plan_error::PlanError;
use std::net::Ipv4Addr;

pub(in crate::overlay) fn unicast(role: &str, raw: &str) -> Result<Ipv4Addr, PlanError> {
    if raw.is_empty() || raw.contains('/') || raw.contains('%') {
        return Err(ip_err(role, raw));
    }
    match raw.parse::<Ipv4Addr>() {
        Ok(ip) if usable(ip) => Ok(ip),
        _ => Err(ip_err(role, raw)),
    }
}
