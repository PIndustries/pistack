// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{port_address, security_rules, Guest};
use fabric::overlay::{PlanPort, PortAddress};
use fabric::{Port, SecurityGroup, Subnet};
use pistack_types::{ApiError, ApiResult};

pub(in crate::overlay) fn port_endpoint(
    guest: &Guest,
    groups: &[SecurityGroup],
    ports: &[Port],
    subnets: &[Subnet],
) -> ApiResult<PlanPort> {
    let address = port_address(&guest.port, subnets).ok_or_else(|| {
        ApiError::unprocessable(format!("port {} has no address", guest.port.meta.id))
    })?;
    if guest.port.spec.mac.is_empty() {
        return Err(ApiError::unprocessable(format!(
            "port {} has no mac",
            guest.port.meta.id
        )));
    }
    let v6 = address.contains(':');
    let (ingress, egress) = if guest.port.spec.port_security {
        security_rules(&guest.port, v6, groups, ports)?
    } else {
        (Vec::new(), Vec::new())
    };
    // Every other fixed address is filtered too, with the rules of its own
    // family, so a dual-stack port is not left open on its second address.
    let mut extra = Vec::new();
    for ip in &guest.port.spec.fixed_ips {
        if ip.to_string() == address {
            continue;
        }
        let (ingress, egress) = if guest.port.spec.port_security {
            security_rules(&guest.port, ip.is_ipv6(), groups, ports)?
        } else {
            (Vec::new(), Vec::new())
        };
        extra.push(PortAddress {
            address: ip.to_string(),
            ingress,
            egress,
        });
    }
    Ok(PlanPort {
        id: guest.port.meta.id.to_string(),
        mac: guest.port.spec.mac.clone(),
        address,
        underlay: guest.underlay.to_string(),
        ingress,
        egress,
        extra,
    })
}
