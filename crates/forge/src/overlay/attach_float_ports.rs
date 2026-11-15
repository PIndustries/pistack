// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{port_endpoint, Guest};
use fabric::overlay::{PlanFloat, PlanPort};
use fabric::{Port, SecurityGroup, Subnet};
use pistack_types::ApiResult;

pub(in crate::overlay) fn attach_float_ports(
    ports: &mut Vec<PlanPort>,
    floating: &[PlanFloat],
    guests: &[Guest],
    groups: &[SecurityGroup],
    all_ports: &[Port],
    subnets: &[Subnet],
) -> ApiResult<()> {
    for float in floating {
        if ports.iter().any(|port| port.address == float.fixed) {
            continue;
        }
        let Some(guest) = guests.iter().find(|guest| {
            guest
                .port
                .spec
                .fixed_ips
                .iter()
                .any(|ip| ip.to_string() == float.fixed)
        }) else {
            continue;
        };
        if ports
            .iter()
            .any(|port| port.id == guest.port.meta.id.to_string())
        {
            continue;
        }
        let mut endpoint = port_endpoint(guest, groups, all_ports, subnets)?;
        endpoint.address = float.fixed.clone();
        ports.push(endpoint);
    }
    Ok(())
}
