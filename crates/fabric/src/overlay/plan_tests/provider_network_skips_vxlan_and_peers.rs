// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::assert_device::assert_device;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::NET_B;
use crate::overlay::types::plan_provider::PlanProvider;

#[test]
pub(in crate::overlay) fn provider_network_skips_vxlan_and_peers() {
    let mut provider = network(NET_A, 0, vec![], vec![]);
    provider.provider = Some(PlanProvider {
        physnet: "eno1".into(),
        vlan: Some(20),
    });
    let overlay = network(NET_B, 2, vec![], vec![]);
    let plan = plan(
        &host("10.0.0.1"),
        &[provider, overlay],
        &["10.0.0.2".into()],
    )
    .unwrap();
    let seg = &plan.networks[0];
    assert_eq!(seg.vni, 0);
    assert_eq!(seg.bridge, "po01a10318a507");
    assert_eq!(seg.vxlan, "");
    let attach = seg.provider.as_ref().unwrap();
    assert_eq!(attach.physnet, "eno1");
    assert_eq!(attach.vlan, Some(20));
    assert!(attach.link.starts_with("pv"), "{}", attach.link);
    assert_eq!(attach.link, "pv01a10318a507");
    assert!(seg.peers.is_empty());
    assert!(!seg.peers.iter().any(|p| p == "0.0.0.0"));
    assert_device(&attach.link);
    assert_eq!(plan.networks[1].vxlan, "vx01a20318b607");
    assert_eq!(plan.networks[1].peers, vec!["10.0.0.2".to_string()]);
    assert!(plan.networks[1].provider.is_none());
}
