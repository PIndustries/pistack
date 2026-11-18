// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

/// A cidr as sent on the wire: a block, or a prefix length alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cidr {
    Block(IpNet),
    Prefix(u8),
}

impl std::str::FromStr for Cidr {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some(digits) = s.strip_prefix('/') {
            return digits
                .parse::<u8>()
                .map(Cidr::Prefix)
                .map_err(|_| format!("'{s}' is not a prefix length"));
        }
        if let Ok(b) = s.parse::<IpNet>() {
            return Ok(Cidr::Block(b));
        }
        // A bare number is a prefix length too; the slash is easy to lose.
        s.parse::<u8>()
            .map(Cidr::Prefix)
            .map_err(|_| format!("'{s}' is neither a cidr nor a prefix length"))
    }
}
