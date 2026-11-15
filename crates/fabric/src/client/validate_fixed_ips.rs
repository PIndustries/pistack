// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::ipam;
use crate::types::*;
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeSet;
use std::net::IpAddr;
impl Client {
    pub(in crate::client) fn validate_fixed_ips(
        &self,
        subnets: &[Subnet],
        ips: &[IpAddr],
        allow_gateway: bool,
    ) -> ApiResult<()> {
        let mut seen = BTreeSet::new();
        for ip in ips {
            if !seen.insert(*ip) {
                return Err(ApiError::bad_request(format!("duplicate fixed ip {ip}")));
            }
            let sub = subnets
                .iter()
                .find(|s| s.spec.cidr.contains(ip))
                .ok_or_else(|| {
                    ApiError::bad_request(format!("{ip} is not in any subnet of this network"))
                })?;
            let used = self.used_addresses(sub)?;
            if allow_gateway && sub.spec.gateway == Some(*ip) {
                if used.contains(ip) {
                    return Err(ApiError::conflict(format!("{ip} is already in use")));
                }
                continue;
            }
            ipam::validate_fixed(&sub.spec, *ip, &used)?;
        }
        Ok(())
    }
}
