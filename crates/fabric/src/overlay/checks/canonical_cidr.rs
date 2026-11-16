// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::plan_error::PlanError;
use ipnet::IpNet;

pub(in crate::overlay) fn canonical_cidr(role: &str, raw: &str) -> Result<String, PlanError> {
    let net: IpNet = raw
        .parse()
        .map_err(|_| PlanError::Invalid(format!("{role} {raw} is not an ip network")))?;
    let canonical = net.trunc().to_string();
    if canonical != raw {
        return Err(PlanError::Invalid(format!("{role} {raw} is not canonical")));
    }
    Ok(canonical)
}
