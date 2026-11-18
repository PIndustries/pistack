// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Endpoint, ServicePort};
use pistack_types::{ApiError, ApiResult};
use std::net::{IpAddr, SocketAddr};

pub(crate) fn endpoint_addrs(
    endpoints: &[Endpoint],
    port: &ServicePort,
) -> ApiResult<Vec<SocketAddr>> {
    let dest = port.target_port.unwrap_or(port.port);
    if dest == 0 {
        return Err(ApiError::unavailable("service target port must not be 0"));
    }
    let mut out = Vec::new();
    for ep in endpoints {
        let ip: IpAddr = match ep.ip.parse() {
            Ok(ip) => ip,
            Err(_) => {
                return Err(ApiError::unavailable("service endpoint is not available"));
            }
        };
        let addr = SocketAddr::new(ip, dest);
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    Ok(out)
}
