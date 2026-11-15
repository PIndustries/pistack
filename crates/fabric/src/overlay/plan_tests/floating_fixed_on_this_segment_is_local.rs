// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::device_hex::device_hex;
use crate::overlay::plan::plan;
use crate::overlay::tests::assert_device::assert_device;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::port;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::types::plan_float::PlanFloat;

#[test]
pub(in crate::overlay) fn floating_fixed_on_this_segment_is_local() {
    let guest = port(
        "port-01a50318-a507-74fd-a86e-5acbda974701",
        "52:54:00:00:00:11",
        "10.0.0.1",
        "10.90.0.11",
    );
    let leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
    let mut net = network(NET_A, 1, vec![leg], vec![guest]);
    let id = "fip-01a60318-e507-74fd-a86e-5acbda974766";
    net.floating = vec![PlanFloat {
        id: id.into(),
        floating: "10.66.66.80".into(),
        fixed: "10.90.0.11".into(),
    }];
    let plan = plan(&host("10.0.0.1"), &[net], &[]).unwrap();
    assert!(plan.networks[0].routers[0].dnat.is_empty());
    let float = &plan.networks[0].floating[0];
    let hex = device_hex(id).unwrap();
    assert_eq!(float.id, id);
    assert_eq!(float.floating, "10.66.66.80");
    assert_eq!(float.fixed, "10.90.0.11");
    assert_eq!(float.netns, format!("nf{hex}"));
    assert_eq!(float.veth_bridge, format!("e{hex}"));
    assert_eq!(float.veth_ns, format!("f{hex}"));
    assert!(float.netns.starts_with("nf"));
    assert!(float.veth_bridge.starts_with('e'));
    assert!(float.veth_ns.starts_with('f'));
    assert_device(&float.netns);
    assert_device(&float.veth_bridge);
    assert_device(&float.veth_ns);
}
