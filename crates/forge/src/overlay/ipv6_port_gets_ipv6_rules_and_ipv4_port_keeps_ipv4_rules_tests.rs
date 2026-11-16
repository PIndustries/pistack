// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{security_rules, sg_port};
use fabric::{Direction, EtherType, Protocol, Remote, Rule, SecurityGroup, SecurityGroupSpec};
use pistack_types::{kind, Meta};

#[test]
fn ipv6_port_gets_ipv6_rules_and_ipv4_port_keeps_ipv4_rules() {
    let mut group = SecurityGroup {
        meta: Meta::new(kind::SECGROUP, "web", None),
        spec: SecurityGroupSpec { rules: vec![] },
        status: Default::default(),
    };
    let gid = group.meta.id.clone();
    group.spec.rules = vec![
        Rule::new(Direction::Ingress, EtherType::IPv4)
            .protocol(Protocol::Tcp)
            .ports(22, 22),
        Rule::new(Direction::Ingress, EtherType::IPv6)
            .protocol(Protocol::Tcp)
            .ports(443, 443),
        Rule::new(Direction::Ingress, EtherType::IPv6)
            .protocol(Protocol::Icmp)
            .remote(Remote::Group(gid.clone())),
        Rule::new(Direction::Ingress, EtherType::IPv6)
            .remote(Remote::Cidr("10.0.0.0/8".parse().unwrap())),
        Rule::new(Direction::Egress, EtherType::IPv6),
    ];
    let me = sg_port(&["10.90.0.5", "fd00::5"], &gid);
    let peer = sg_port(&["10.90.0.6", "fd00::6"], &gid);
    let ports = vec![me.clone(), peer];
    let groups = vec![group];

    let (ing, eg) = security_rules(&me, true, &groups, &ports).unwrap();
    assert!(ing
        .iter()
        .any(|r| r.proto == "tcp" && r.port_min == 443 && r.cidr == "::/0"));
    assert!(ing
        .iter()
        .any(|r| r.proto == "icmp" && r.cidr == "fd00::6/128"));
    assert!(ing.iter().all(|r| r.cidr.contains(':')), "{ing:?}");
    assert!(ing.iter().all(|r| r.port_min != 22));
    assert_eq!(eg.len(), 1);
    assert_eq!(eg[0].cidr, "::/0");

    let (ing4, eg4) = security_rules(&me, false, &groups, &ports).unwrap();
    assert_eq!(ing4.len(), 1);
    assert_eq!(ing4[0].port_min, 22);
    assert_eq!(ing4[0].cidr, "0.0.0.0/0");
    assert!(eg4.is_empty());
}
