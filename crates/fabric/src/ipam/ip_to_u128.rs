// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

pub(crate) fn ip_to_u128(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(v4) => u32::from(v4) as u128,
        IpAddr::V6(v6) => u128::from(v6),
    }
}
