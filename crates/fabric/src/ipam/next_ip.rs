// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

use super::ip_to_u128::ip_to_u128;
use super::u128_to_ip::u128_to_ip;

/// The address after `ip`, or `None` at the top of the address family.
pub fn next_ip(ip: IpAddr) -> Option<IpAddr> {
    u128_to_ip(ip_to_u128(ip).checked_add(1)?, ip.is_ipv4())
}
