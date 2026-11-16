// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::consts::PLAN_VERSION;
use crate::overlay::consts::VXLAN_PORT;
use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::NET_B;

#[test]
pub(in crate::overlay) fn two_networks_get_distinct_vnis_and_devices() {
    assert_eq!(VXLAN_PORT, 4789);
    let nets = [
        network(NET_A, 1, vec![], vec![]),
        network(NET_B, 2, vec![], vec![]),
    ];
    let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    assert_eq!(plan.version, PLAN_VERSION);
    assert_eq!(plan.host, "10.0.0.1");
    assert_eq!(plan.networks.len(), 2);
    assert_ne!(plan.networks[0].vni, plan.networks[1].vni);
    assert_eq!(plan.networks[0].vni, 1);
    assert_eq!(plan.networks[1].vni, 2);
    assert_eq!(plan.networks[0].bridge, "po01a10318a507");
    assert_eq!(plan.networks[0].vxlan, "vx01a10318a507");
    assert_eq!(plan.networks[1].bridge, "po01a20318b607");
    assert_eq!(plan.networks[1].vxlan, "vx01a20318b607");
    assert_ne!(plan.networks[0].bridge, plan.networks[1].bridge);
    assert_ne!(plan.networks[0].vxlan, plan.networks[1].vxlan);
    assert_eq!(plan.networks[0].mtu, 1450);
    assert_eq!(plan.networks[1].mtu, 1450);
}
