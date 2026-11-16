// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use ipnet::IpNet;

use super::address_entry::address_entry;
use crate::types::Port;

/// One pair as OVN entries: the addresses entry, and the port_security entry.
pub(in crate::backend::ovn) struct PairRows {
    /// What `addresses` gains, if anything. A single address is listed bare;
    /// a prefix has no place in `addresses` at all, and only its MAC is
    /// listed — once, and only when it is not the port's own — so the switch
    /// knows where that MAC lives without the port claiming to own every
    /// address of the prefix.
    pub(in crate::backend::ovn) addresses: Option<String>,
    /// What `port_security` gains: the pair as it was asked for, mask and all.
    pub(in crate::backend::ovn) security: String,
}

/// A port's allowed address pairs, split the way the two columns want them.
/// A pair that names no MAC — or one fabric could not parse, which the API
/// refuses long before a pair reaches here — rides on the port's own MAC.
pub(in crate::backend::ovn) fn pair_rows(port: &Port) -> Vec<PairRows> {
    let own = port.spec.mac.trim();
    crate::ipam::pair_macs(port)
        .into_iter()
        .map(|(mac, net)| {
            let mac = mac.unwrap_or_else(|| own.to_string());
            let security = address_entry(&mac, [net.to_string()]);
            let full = match net {
                IpNet::V4(_) => 32,
                IpNet::V6(_) => 128,
            };
            let single = net.prefix_len() == full;
            // The port already answers for its own MAC, so a prefix under it
            // teaches the switch nothing and lists no new address.
            let addresses = if single {
                Some(address_entry(&mac, [net.addr().to_string()]))
            } else if mac == own {
                None
            } else {
                Some(mac)
            };
            PairRows {
                addresses,
                security,
            }
        })
        .collect()
}
