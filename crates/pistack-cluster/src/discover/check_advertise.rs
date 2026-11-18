// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

use super::discover_error::DiscoverError;

/// Refuse unspecified, multicast, broadcast, and public addresses.
pub fn check_advertise(ip: IpAddr) -> Result<(), DiscoverError> {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_private() && !v4.is_unspecified() && !v4.is_broadcast() && !v4.is_multicast() {
                Ok(())
            } else {
                Err(DiscoverError::Address(v4.to_string()))
            }
        }
        IpAddr::V6(v6) => Err(DiscoverError::Address(v6.to_string())),
    }
}
