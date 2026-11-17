// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! 10/8, 172.16/12 and 192.168/16.

use std::net::Ipv4Addr;

pub(super) fn is_rfc1918(v4: Ipv4Addr) -> bool {
    let [a, b, _, _] = v4.octets();
    a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168)
}
