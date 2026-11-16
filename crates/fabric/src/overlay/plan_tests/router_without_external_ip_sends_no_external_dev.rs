// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::types::overlay_plan::OverlayPlan;

#[test]
pub(in crate::overlay) fn router_without_external_ip_sends_no_external_dev() {
    let mut leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
    leg.external_dev = "external".into();
    let nets = [network(NET_A, 3, vec![leg], vec![])];
    let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    let ns = &plan.networks[0].routers[0];
    assert!(ns.external_ip.is_none());
    assert!(!ns.snat);
    assert!(ns.external_dev.is_empty());
    let value = serde_json::to_value(&plan).unwrap();
    let doc = &value["networks"][0]["routers"][0];
    assert!(doc.get("external_dev").is_none(), "{doc}");
    let back: OverlayPlan = serde_json::from_value(value).unwrap();
    assert!(back.networks[0].routers[0].external_dev.is_empty());
}
