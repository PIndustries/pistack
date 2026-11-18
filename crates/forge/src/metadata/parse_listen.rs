// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;

/// Refuse an unspecified address. `0.0.0.0` and `::` would answer on the
/// public NIC, and this service has no credentials of its own.
pub fn parse_listen(text: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = text
        .parse()
        .map_err(|_| format!("metadata listen {text} is not an address"))?;
    if addr.ip().is_unspecified() {
        return Err(format!("metadata listen {addr} must not be 0.0.0.0 or ::"));
    }
    Ok(addr)
}
