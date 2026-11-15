// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{port_endpoint, sg_port, Guest};
use fabric::{Direction, EtherType, Protocol, Rule, SecurityGroup, SecurityGroupSpec};
use pistack_types::{kind, Meta};

#[test]
fn dual_stack_port_plans_rules_for_both_addresses() {
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
    ];
    let mut port = sg_port(&["10.90.0.5", "fd00::5"], &gid);
    port.spec.mac = "52:54:00:00:00:55".into();
    let guest = Guest {
        port: port.clone(),
        underlay: "10.1.0.1".parse().unwrap(),
        node: "a".into(),
    };
    let groups = vec![group];
    let got = port_endpoint(&guest, &groups, &[port], &[]).unwrap();
    assert_eq!(got.address, "10.90.0.5");
    assert_eq!(got.ingress.len(), 1);
    assert_eq!(got.ingress[0].port_min, 22);
    assert_eq!(got.extra.len(), 1);
    assert_eq!(got.extra[0].address, "fd00::5");
    assert_eq!(got.extra[0].ingress.len(), 1);
    assert_eq!(got.extra[0].ingress[0].port_min, 443);
    assert_eq!(got.extra[0].ingress[0].cidr, "::/0");
}
