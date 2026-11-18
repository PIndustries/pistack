// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use serde::{Deserialize, Deserializer};
use std::net::IpAddr;

/// One accepted spelling of a pair's address: a CIDR, or a bare address
/// meaning that one address of its family. Anything else is reported by the
/// reader that checks the rest of the spec, not here.
pub(crate) fn deserialize_ip_or_cidr<'de, D>(d: D) -> Result<IpNet, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    s.parse::<IpNet>()
        .or_else(|_| s.parse::<IpAddr>().map(IpNet::from))
        .map_err(|_| {
            serde::de::Error::invalid_value(serde::de::Unexpected::Str(&s), &"an address or cidr")
        })
}
