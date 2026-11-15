// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::Ipv4Addr;

pub(in crate::overlay) fn shared_nat(address: &str) -> bool {
    let Ok(ip) = address.parse::<Ipv4Addr>() else {
        return false;
    };
    let o = ip.octets();
    o[0] == 10 && o[1] == 88 && o[2] == 0
}
