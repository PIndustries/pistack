// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::svc;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::NET_B;
use crate::overlay::tests::test_support::RTR_A;

#[test]
pub(in crate::overlay) fn router_legs_merge_their_service_rules() {
    let mut leg_a = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
    leg_a.services = vec![svc("svc-web", "10.96.0.10", 80, &[("10.90.0.5", 8080)])];
    let mut leg_b = router(RTR_A, "10.91.0.1", "10.91.0.0/24");
    leg_b.services = vec![
        svc("svc-web", "10.96.0.10", 80, &[("10.91.0.6", 8080)]),
        svc("svc-idle", "10.96.0.11", 53, &[]),
    ];
    let shared = svc(
        "svc-web",
        "10.96.0.10",
        80,
        &[("10.90.0.5", 8080), ("10.91.0.6", 8080)],
    );
    let nets = [
        network(NET_A, 3, vec![leg_a], vec![]),
        network(NET_B, 4, vec![leg_b], vec![]),
    ];
    let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    let ns = plan
        .networks
        .iter()
        .flat_map(|s| &s.routers)
        .next()
        .unwrap();
    assert_eq!(ns.services.len(), 2);
    assert_eq!(ns.services[0], shared);
    assert!(ns.services[1].backends.is_empty());
    let json = serde_json::to_value(ns).unwrap();
    assert_eq!(json["services"][0]["backends"][1]["address"], "10.91.0.6");
}
