// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{member_cidrs, span};
use fabric::overlay::SgRule;
use fabric::{Direction, EtherType, Port, Protocol, Remote, SecurityGroup};
use pistack_types::ApiResult;

/// Security group rules for one port, in the family of its planned address.
///
/// The helper filters traffic to that one address, so an IPv4 port gets the
/// IPv4 rules and an IPv6 port gets the IPv6 rules (applied with ip6tables).
/// A rule for the other family has nothing to match and is left out.
pub(in crate::overlay) fn security_rules(
    port: &Port,
    v6: bool,
    groups: &[SecurityGroup],
    ports: &[Port],
) -> ApiResult<(Vec<SgRule>, Vec<SgRule>)> {
    let mut ingress = Vec::new();
    let mut egress = Vec::new();
    for id in &port.spec.security_groups {
        let Some(group) = groups.iter().find(|g| &g.meta.id == id) else {
            continue;
        };
        for rule in &group.spec.rules {
            let want = if v6 { EtherType::IPv6 } else { EtherType::IPv4 };
            if rule.ethertype != want {
                continue;
            }
            let (proto, port_min, port_max) = match rule.protocol {
                Some(Protocol::Tcp) => {
                    let (min, max) = span(rule.port_range.map(|p| (p.min, p.max)));
                    ("tcp", min, max)
                }
                Some(Protocol::Udp) => {
                    let (min, max) = span(rule.port_range.map(|p| (p.min, p.max)));
                    ("udp", min, max)
                }
                Some(Protocol::Icmp) => ("icmp", 0, 0),
                Some(Protocol::Any) | None => ("any", 0, 0),
            };
            let cidrs = match &rule.remote {
                Remote::Any if v6 => vec!["::/0".to_string()],
                Remote::Any => vec!["0.0.0.0/0".to_string()],
                Remote::Cidr(net) => {
                    let text = net.to_string();
                    if text.contains(':') != v6 {
                        continue;
                    }
                    vec![text]
                }
                Remote::Group(gid) => member_cidrs(gid, v6, ports),
            };
            for cidr in cidrs {
                let item = SgRule {
                    proto: proto.to_string(),
                    port_min,
                    port_max,
                    cidr,
                };
                match rule.direction {
                    Direction::Ingress => ingress.push(item),
                    Direction::Egress => egress.push(item),
                }
            }
        }
    }
    Ok((ingress, egress))
}
