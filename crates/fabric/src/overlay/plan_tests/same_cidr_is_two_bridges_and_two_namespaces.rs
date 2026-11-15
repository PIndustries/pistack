// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::NET_B;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::tests::test_support::RTR_B;

#[test]
pub(in crate::overlay) fn same_cidr_is_two_bridges_and_two_namespaces() {
    let nets = [
        network(
            NET_A,
            7,
            vec![router(RTR_A, "10.90.0.1", "10.90.0.0/24")],
            vec![],
        ),
        network(
            NET_B,
            8,
            vec![router(RTR_B, "10.90.0.1", "10.90.0.0/24")],
            vec![],
        ),
    ];
    let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    assert_eq!(plan.networks.len(), 2);
    assert_ne!(plan.networks[0].vni, plan.networks[1].vni);
    assert_ne!(plan.networks[0].bridge, plan.networks[1].bridge);
    assert_ne!(plan.networks[0].vxlan, plan.networks[1].vxlan);
    let a = &plan.networks[0].routers[0];
    let b = &plan.networks[1].routers[0];
    assert_ne!(a.netns, b.netns);
    assert_eq!(a.interfaces[0].gateway, "10.90.0.1");
    assert_eq!(b.interfaces[0].gateway, "10.90.0.1");
    assert_eq!(a.interfaces[0].cidr, "10.90.0.0/24");
    assert_eq!(b.interfaces[0].cidr, "10.90.0.0/24");
    assert!(!a.snat);
    assert!(!b.snat);
}
