// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

use crate::realizer::node_port_address_env::NODE_PORT_ADDRESS_ENV;

/// `PISTACK_NODE_PORT_ADDRESS` when it is a loopback or private address,
/// otherwise loopback.
pub(crate) fn node_port_address() -> IpAddr {
    let loopback = IpAddr::from([127, 0, 0, 1]);
    let Ok(raw) = std::env::var(NODE_PORT_ADDRESS_ENV) else {
        return loopback;
    };
    match raw.trim().parse::<IpAddr>() {
        Ok(ip) if relay::bind::allowed(ip) => ip,
        _ => {
            tracing::warn!(
                value = %raw,
                "loom: {NODE_PORT_ADDRESS_ENV} must be a loopback or private address; node ports bind on 127.0.0.1"
            );
            loopback
        }
    }
}
