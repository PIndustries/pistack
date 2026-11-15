// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::device_hex::device_hex;
use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::NET_B;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::types::dnat_rule::DnatRule;

#[test]
pub(in crate::overlay) fn one_router_two_networks_is_one_namespace() {
    let mut leg_a = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
    leg_a.external_ip = Some("10.66.66.9".into());
    leg_a.external_dev = "wg-fleet".into();
    leg_a.metadata = true;
    leg_a.dnat = vec![DnatRule {
        id: "fip1".into(),
        floating: "10.66.66.80".into(),
        fixed: "10.90.0.5".into(),
    }];
    let mut leg_b = router(RTR_A, "10.91.0.1", "10.91.0.0/24");
    leg_b.external_ip = Some("10.66.66.9".into());
    leg_b.external_dev = "wg-fleet".into();
    leg_b.metadata = true;
    let nets = [
        network(NET_A, 3, vec![leg_a], vec![]),
        network(NET_B, 4, vec![leg_b], vec![]),
    ];
    let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    let nss: Vec<_> = plan.networks.iter().flat_map(|s| &s.routers).collect();
    assert_eq!(nss.len(), 1);
    let ns = nss[0];
    assert_eq!(ns.interfaces.len(), 2);
    assert_eq!(ns.interfaces[0].network_id, NET_A);
    assert_eq!(ns.interfaces[1].network_id, NET_B);
    assert_eq!(ns.interfaces[0].gateway, "10.90.0.1");
    assert_eq!(ns.interfaces[1].gateway, "10.91.0.1");
    let hex = device_hex(RTR_A).unwrap();
    let a = device_hex(NET_A).unwrap();
    let b = device_hex(NET_B).unwrap();
    assert_eq!(ns.netns, format!("ns{hex}"));
    assert_eq!(ns.external_veth_root, format!("c{hex}"));
    assert_eq!(ns.external_veth_ns, format!("d{hex}"));
    assert_eq!(
        ns.interfaces[0].veth_bridge,
        format!("a{}{}", &a[..6], &hex[..6])
    );
    assert_eq!(
        ns.interfaces[0].veth_ns,
        format!("b{}{}", &a[..6], &hex[..6])
    );
    assert_eq!(
        ns.interfaces[1].veth_bridge,
        format!("a{}{}", &b[..6], &hex[..6])
    );
    assert!(ns.snat);
    assert!(ns.metadata);
    assert_eq!(ns.external_dev, "wg-fleet");
    assert_eq!(ns.external_ip.as_deref(), Some("10.66.66.9"));
    assert_eq!(ns.dnat.len(), 1);
    assert_eq!(ns.dnat[0].floating, "10.66.66.80");
    assert_eq!(ns.dnat[0].fixed, "10.90.0.5");
}
