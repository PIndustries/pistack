// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The v6 rule: loopback or unique local, unless unspecified, multicast or
//! link-local.

use std::net::Ipv6Addr;

pub(super) fn v6_allowed(v6: Ipv6Addr) -> bool {
    if v6.is_unspecified() || v6.is_multicast() || v6.is_unicast_link_local() {
        return false;
    }
    v6.is_loopback() || v6.is_unique_local()
}
