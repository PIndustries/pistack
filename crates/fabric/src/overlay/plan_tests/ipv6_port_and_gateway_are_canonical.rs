// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::port;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::NET_B;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::types::sg_rule::SgRule;

#[test]
pub(in crate::overlay) fn ipv6_port_and_gateway_are_canonical() {
    let mut guest = port(
        "port-01a50318-a507-74fd-a86e-5acbda974701",
        "52:54:00:00:00:11",
        "10.0.0.1",
        "2001:0db8:0000:0000:0000:0000:0000:0010",
    );
    guest.ingress = vec![SgRule {
        proto: "tcp".into(),
        port_min: 22,
        port_max: 22,
        cidr: "2001:db8::/64".into(),
    }];
    let mut leg = router(
        RTR_A,
        "2001:0db8:0000:0001:0000:0000:0000:0001",
        "2001:0db8:0000:0001::/64",
    );
    leg.prefix = 64;
    let nets = [
        network(NET_A, 1, vec![], vec![guest]),
        network(NET_B, 2, vec![leg], vec![]),
    ];
    let got = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    assert_eq!(got.networks[0].ports[0].address, "2001:db8::10");
    assert_eq!(got.networks[0].ports[0].ingress[0].cidr, "2001:db8::/64");
    let iface = &got.networks[1].routers[0].interfaces[0];
    assert_eq!(iface.gateway, "2001:db8:0:1::1");
    assert_eq!(iface.prefix, 64);
    assert_eq!(iface.cidr, "2001:db8:0:1::/64");
    assert_eq!(got.host, "10.0.0.1");

    let mut bad = port(
        "port-01a50318-a507-74fd-a86e-5acbda974702",
        "52:54:00:00:00:12",
        "10.0.0.1",
        "10.90.0.12",
    );
    bad.egress = vec![SgRule {
        proto: "any".into(),
        port_min: 0,
        port_max: 0,
        cidr: "2001:0db8::/64".into(),
    }];
    let nets = [network(NET_A, 1, vec![], vec![bad])];
    assert!(plan(&host("10.0.0.1"), &nets, &[]).is_err());
}
