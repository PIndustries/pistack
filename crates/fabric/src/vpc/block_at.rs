// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::{IpNet, Ipv4Net, Ipv6Net};

/// The block of `prefix` addresses starting at `at`, in the family of `cidr`.
pub(crate) fn block_at(cidr: IpNet, at: u128, prefix: u16) -> Option<IpNet> {
    match cidr {
        IpNet::V4(_) => {
            let v4 = u32::try_from(at).ok()?;
            Ipv4Net::new(v4.into(), prefix as u8).ok().map(IpNet::V4)
        }
        IpNet::V6(_) => Ipv6Net::new(at.into(), prefix as u8).ok().map(IpNet::V6),
    }
}
