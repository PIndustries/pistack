// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::ApiResult;
use std::collections::BTreeSet;
use std::net::IpAddr;
impl Client {
    /// Addresses held in a subnet: port fixed IPs and floating IPs that fall
    /// inside its CIDR.
    pub fn used_addresses(&self, subnet: &Subnet) -> ApiResult<BTreeSet<IpAddr>> {
        let cidr = subnet.spec.cidr;
        let mut used = BTreeSet::new();
        for p in self.ports_on_network(&subnet.spec.network)? {
            used.extend(
                p.spec
                    .fixed_ips
                    .iter()
                    .copied()
                    .filter(|ip| cidr.contains(ip)),
            );
        }
        for f in self.floating_ips().list(None).map_err(store_err)? {
            if f.spec.network == subnet.spec.network {
                if let Some(a) = f.spec.address {
                    if cidr.contains(&a) {
                        used.insert(a);
                    }
                }
            }
        }
        Ok(used)
    }
}
