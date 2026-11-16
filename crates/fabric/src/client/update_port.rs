// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::ipam;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};
use std::net::IpAddr;
impl Client {
    pub fn update_port(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Port) -> ApiResult<()>,
    ) -> ApiResult<Port> {
        let cur = self.ports().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        validate_pairs(&next.spec)?;
        if next.spec.network != cur.spec.network || next.spec.subnet != cur.spec.subnet {
            return Err(ApiError::bad_request(
                "port network and subnet are immutable",
            ));
        }
        if next.spec.mac != cur.spec.mac {
            next.spec.mac = ipam::format_mac(ipam::parse_mac(&next.spec.mac)?);
            if next.spec.mac != cur.spec.mac
                && self
                    .macs_on_network(&cur.spec.network)?
                    .contains(&next.spec.mac)
            {
                return Err(ApiError::conflict(format!(
                    "mac {} is already in use on this network",
                    next.spec.mac
                )));
            }
        }
        if next.spec.fixed_ips != cur.spec.fixed_ips {
            let subnets = self.subnets_of_network(&cur.spec.network)?;
            let added: Vec<IpAddr> = next
                .spec
                .fixed_ips
                .iter()
                .copied()
                .filter(|ip| !cur.spec.fixed_ips.contains(ip))
                .collect();
            self.validate_fixed_ips(&subnets, &added, cur.is_router_port())?;
        }
        if !next.spec.port_security {
            next.spec.security_groups.clear();
        }
        self.validate_security_groups(&next.meta, &next.spec.security_groups)?;
        self.ports().update(next, expected).map_err(store_err)
    }
}
