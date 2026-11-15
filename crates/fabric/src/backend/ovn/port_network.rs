// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_types::ApiError;

use crate::types::{Port, Subnet};
use pistack_types::ApiResult;

/// `<address>/<prefix>` for a router port on `subnet`: the port's address
/// inside the subnet, else the subnet gateway.
pub(in crate::backend::ovn) fn port_network(port: &Port, subnet: &Subnet) -> ApiResult<String> {
    let cidr = subnet.spec.cidr;
    let ip = port
        .spec
        .fixed_ips
        .iter()
        .copied()
        .find(|ip| cidr.contains(ip))
        .or(subnet.spec.gateway)
        .ok_or_else(|| {
            ApiError::unavailable(format!(
                "router port {} has no address on subnet {}",
                port.meta.id, subnet.meta.id
            ))
        })?;
    Ok(format!("{ip}/{}", cidr.prefix_len()))
}
