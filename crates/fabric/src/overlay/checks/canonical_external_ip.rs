// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_ip::canonical_ip;
use crate::overlay::checks::is_private_ula::is_private_ula;
use crate::overlay::types::plan_error::PlanError;
use std::net::IpAddr;

pub(in crate::overlay) fn canonical_external_ip(
    raw: &Option<String>,
) -> Result<Option<String>, PlanError> {
    match raw {
        None => Ok(None),
        Some(s) => match canonical_ip("external ip", s)? {
            IpAddr::V4(ip) => Ok(Some(ip.to_string())),
            IpAddr::V6(ip) if is_private_ula(ip) => Ok(Some(ip.to_string())),
            IpAddr::V6(ip) => Err(PlanError::Ip(format!(
                "external ip {ip} is not a private ipv6 address"
            ))),
        },
    }
}
