// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use serde::{Deserialize, Serialize};

use super::deserialize_ip_or_cidr::deserialize_ip_or_cidr;

/// An `(address, mac)` the port may also carry. Without one a port only
/// ever moves traffic for its own MAC and its fixed IPs, so an address
/// nobody owns can be moved onto a machine by pointing it at that machine's
/// port: a virtual address two machines share, an address kept while a
/// machine is replaced. The MAC is the one seen on the wire, which defaults
/// to the port's own.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedAddressPair {
    /// A single address or a CIDR. A bare address means that one address.
    #[serde(deserialize_with = "deserialize_ip_or_cidr")]
    pub ip_address: IpNet,
    /// Omitted means the port's own MAC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
}
