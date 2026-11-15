// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::assert_device::assert_device;
use crate::overlay::tests::is_bridge::is_bridge;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::NET_B;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::tests::test_support::RTR_B;

#[test]
pub(in crate::overlay) fn planned_names_are_not_hostnat_bridges() {
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
            vec![router(RTR_B, "10.91.0.1", "10.91.0.0/24")],
            vec![],
        ),
    ];
    let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    assert!(!plan.networks.is_empty());
    for seg in &plan.networks {
        assert!(
            is_bridge(&seg.bridge),
            "{} does not match ^po[0-9a-f]{{12}}$",
            seg.bridge
        );
        assert_device(&seg.bridge);
        assert_device(&seg.vxlan);
        for r in &seg.routers {
            assert_device(&r.netns);
            assert_device(&r.external_veth_root);
            assert_device(&r.external_veth_ns);
            for iface in &r.interfaces {
                assert_device(&iface.veth_bridge);
                assert_device(&iface.veth_ns);
            }
        }
    }
}
