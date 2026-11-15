// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::pub_consts::METADATA_ADDRESS;
use super::{braces::braces, server_mac::server_mac};
use crate::types::{Network, Subnet};
use std::net::IpAddr;

pub(in crate::backend::ovn) fn dhcp_options(
    net: &Network,
    subnet: &Subnet,
) -> Vec<(String, String)> {
    let s = &subnet.spec;
    let mac = server_mac(&subnet.meta.id);
    let mut o: Vec<(String, String)> = Vec::new();
    let dns = (!s.dns_nameservers.is_empty())
        .then(|| braces(s.dns_nameservers.iter().map(IpAddr::to_string)));
    if s.cidr.addr().is_ipv4() {
        let server = s.gateway.or_else(|| s.cidr.hosts().next());
        if let Some(server) = server {
            o.push(("server_id".into(), server.to_string()));
        }
        o.push(("server_mac".into(), mac));
        o.push(("lease_time".into(), "43200".into()));
        if let Some(gw) = s.gateway {
            o.push(("router".into(), gw.to_string()));
        }
        if let Some(dns) = dns {
            o.push(("dns_server".into(), dns));
        }
        o.push(("mtu".into(), net.spec.mtu.to_string()));
        // Router 0.0.0.0 is an on-link route (RFC 3442): the metadata port
        // sits on the guest's own segment, so this holds with or without a
        // gateway. A client given this option ignores the router option,
        // which is why the default route is repeated here.
        let mut routes = vec![format!("{METADATA_ADDRESS}/32,0.0.0.0")];
        routes.extend(
            s.host_routes
                .iter()
                .map(|r| format!("{},{}", r.destination, r.nexthop)),
        );
        if let Some(gw) = s.gateway {
            routes.push(format!("0.0.0.0/0,{gw}"));
        }
        o.push(("classless_static_route".into(), braces(routes)));
    } else {
        o.push(("server_id".into(), mac));
        if let Some(dns) = dns {
            o.push(("dns_server".into(), dns));
        }
    }
    o
}
