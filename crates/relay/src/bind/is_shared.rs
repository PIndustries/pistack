// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Carrier-grade NAT, 100.64.0.0/10.

use std::net::Ipv4Addr;

pub(super) fn is_shared(v4: Ipv4Addr) -> bool {
    let [a, b, _, _] = v4.octets();
    a == 100 && (64..128).contains(&b)
}
