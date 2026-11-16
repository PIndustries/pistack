// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::port;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::types::port_address::PortAddress;
use crate::overlay::types::sg_rule::SgRule;

#[test]
pub(in crate::overlay) fn dual_stack_port_carries_its_other_address_and_rules() {
    let mut guest = port(
        "port-01a50318-a507-74fd-a86e-5acbda974701",
        "52:54:00:00:00:11",
        "10.0.0.1",
        "10.90.0.5",
    );
    guest.extra = vec![PortAddress {
        address: "2001:0db8::5".into(),
        ingress: vec![SgRule {
            proto: "tcp".into(),
            port_min: 443,
            port_max: 443,
            cidr: "::/0".into(),
        }],
        egress: vec![],
    }];
    let nets = [network(NET_A, 1, vec![], vec![guest.clone()])];
    let got = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    let out = &got.networks[0].ports[0];
    assert_eq!(out.extra.len(), 1);
    assert_eq!(out.extra[0].address, "2001:db8::5");
    assert_eq!(out.extra[0].ingress[0].port_min, 443);
    let text = serde_json::to_string(&got).unwrap();
    assert!(text.contains("\"extra\""), "{text}");

    let mut wrong = guest.clone();
    wrong.extra[0].ingress[0].cidr = "10.0.0.0/8".into();
    let nets = [network(NET_A, 1, vec![], vec![wrong])];
    assert!(plan(&host("10.0.0.1"), &nets, &[]).is_err());

    let mut twice = guest;
    twice.extra[0].address = "10.90.0.5".into();
    twice.extra[0].ingress.clear();
    let nets = [network(NET_A, 1, vec![], vec![twice])];
    assert!(plan(&host("10.0.0.1"), &nets, &[]).is_err());
}
