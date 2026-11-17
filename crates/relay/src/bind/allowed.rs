// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The question a forwarder or the HTTP proxy asks of a bind address, sent
//! to the v4 or v6 rule as appropriate.

use super::{v4_allowed::v4_allowed, v6_allowed::v6_allowed};
use std::net::IpAddr;

/// True when `ip` may be bound by a forwarder or the HTTP proxy.
pub fn allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_allowed(v4),
        IpAddr::V6(v6) => v6_allowed(v6),
    }
}
