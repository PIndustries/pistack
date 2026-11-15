// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::{Direction, EtherType, Remote};

use super::{acl::Acl, consts::RULE_PRIORITY, group_name::group_name};
use crate::types::{Protocol, Rule};

/// The ACL for one rule on group `group`, or `None` when the rule cannot
/// match anything (a remote address of the other family).
pub(in crate::backend::ovn) fn rule_acl(group: &str, rule: &Rule) -> Option<Acl> {
    let fam = match rule.ethertype {
        EtherType::IPv4 => "ip4",
        EtherType::IPv6 => "ip6",
    };
    let (direction, port_side, addr_side) = match rule.direction {
        Direction::Ingress => ("to-lport", "outport", "src"),
        Direction::Egress => ("from-lport", "inport", "dst"),
    };
    let mut m = format!("{port_side} == @{group} && {fam}");
    match &rule.remote {
        Remote::Any => {}
        Remote::Cidr(c) => {
            if c.addr().is_ipv4() != (fam == "ip4") {
                return None;
            }
            m.push_str(&format!(" && {fam}.{addr_side} == {}", c.trunc()));
        }
        Remote::Group(g) => {
            m.push_str(&format!(
                " && {fam}.{addr_side} == ${}_{fam}",
                group_name(g)
            ));
        }
    }
    match rule.protocol {
        Some(p @ (Protocol::Tcp | Protocol::Udp)) => {
            let l4 = if p == Protocol::Tcp { "tcp" } else { "udp" };
            m.push_str(&format!(" && {l4}"));
            if let Some(r) = rule.port_range {
                if r.min == r.max {
                    m.push_str(&format!(" && {l4}.dst == {}", r.min));
                } else {
                    m.push_str(&format!(
                        " && {l4}.dst >= {} && {l4}.dst <= {}",
                        r.min, r.max
                    ));
                }
            }
        }
        Some(Protocol::Icmp) => m.push_str(if fam == "ip4" {
            " && icmp4"
        } else {
            " && icmp6"
        }),
        Some(Protocol::Any) | None => {}
    }
    Some(Acl {
        direction,
        priority: RULE_PRIORITY,
        matches: m,
        action: "allow-related",
        rule: Some(rule.id.to_string()),
    })
}
