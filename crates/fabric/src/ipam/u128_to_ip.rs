// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) fn u128_to_ip(n: u128, v4: bool) -> Option<IpAddr> {
    if v4 {
        u32::try_from(n).ok().map(|x| IpAddr::V4(Ipv4Addr::from(x)))
    } else {
        Some(IpAddr::V6(Ipv6Addr::from(n)))
    }
}
