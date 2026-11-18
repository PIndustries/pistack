// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

/// How many addresses a block holds.
pub(crate) fn span(cidr: IpNet) -> u128 {
    let free = match cidr {
        IpNet::V4(n) => u128::from(32 - n.prefix_len()),
        IpNet::V6(n) => u128::from(128 - n.prefix_len()),
    };
    if free >= 128 {
        u128::MAX
    } else {
        1u128 << free
    }
}
