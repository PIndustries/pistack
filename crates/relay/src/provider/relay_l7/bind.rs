// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The proxy's bind rules: which vip addresses it will hold, and where a
//! balancer's socket goes.

use crate::types::Balancer;
use pistack_types::{ApiError, ApiResult};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub(super) fn vip_allowed(ip: IpAddr) -> bool {
    crate::bind::allowed(ip)
}

pub(super) fn bind_target(lb: &Balancer) -> ApiResult<SocketAddr> {
    let ip = match lb.spec.vip_address {
        None => IpAddr::V4(Ipv4Addr::LOCALHOST),
        Some(ip) if vip_allowed(ip) => ip,
        Some(ip) => {
            return Err(ApiError::bad_request(format!(
                "http refuses vip {ip}; only a loopback or private address can be bound"
            )));
        }
    };
    Ok(SocketAddr::new(ip, lb.spec.vip_port.unwrap_or(0)))
}
