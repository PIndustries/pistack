// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::Ipv6Addr;

pub(in crate::overlay) fn usable_v6(ip: Ipv6Addr) -> bool {
    !ip.is_unspecified() && !ip.is_multicast()
}
