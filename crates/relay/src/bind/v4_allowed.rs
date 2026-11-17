// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The v4 rule: loopback or RFC 1918, unless the address is unspecified,
//! multicast, broadcast, link-local, or carrier-grade NAT.

use super::is_rfc1918::is_rfc1918;
use super::is_shared::is_shared;
use std::net::Ipv4Addr;

pub(super) fn v4_allowed(v4: Ipv4Addr) -> bool {
    if v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
        || v4.is_link_local()
        || is_shared(v4)
    {
        return false;
    }
    v4.is_loopback() || is_rfc1918(v4)
}
