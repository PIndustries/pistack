// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_ip::canonical_ip;
use crate::overlay::types::plan_error::PlanError;
use ipnet::Ipv4Net;
use ipnet::Ipv6Net;
use std::net::IpAddr;

pub(in crate::overlay) fn gateway_in_cidr(
    gateway: &str,
    prefix: u8,
    cidr: &str,
) -> Result<(String, String), PlanError> {
    match canonical_ip("gateway", gateway)? {
        IpAddr::V4(gw) => {
            if !(1..=30).contains(&prefix) {
                return Err(PlanError::Invalid(format!(
                    "prefix {prefix} is outside 1..=30"
                )));
            }
            let net: Ipv4Net = cidr
                .parse()
                .map_err(|_| PlanError::Invalid(format!("cidr {cidr} is not an ipv4 network")))?;
            if net.prefix_len() != prefix {
                return Err(PlanError::Invalid(format!(
                    "prefix {prefix} does not match cidr {cidr}"
                )));
            }
            if !net.contains(&gw) || gw == net.network() || gw == net.broadcast() {
                return Err(PlanError::Invalid(format!(
                    "gateway {gateway} is outside {cidr}"
                )));
            }
            Ok((gw.to_string(), net.trunc().to_string()))
        }
        IpAddr::V6(gw) => {
            if !(1..=128).contains(&prefix) {
                return Err(PlanError::Invalid(format!(
                    "prefix {prefix} is outside 1..=128"
                )));
            }
            let net: Ipv6Net = cidr
                .parse()
                .map_err(|_| PlanError::Invalid(format!("cidr {cidr} is not an ipv6 network")))?;
            if net.prefix_len() != prefix {
                return Err(PlanError::Invalid(format!(
                    "prefix {prefix} does not match cidr {cidr}"
                )));
            }
            if !net.contains(&gw) {
                return Err(PlanError::Invalid(format!(
                    "gateway {gateway} is outside {cidr}"
                )));
            }
            Ok((gw.to_string(), net.trunc().to_string()))
        }
    }
}
