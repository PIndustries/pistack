// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::port;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::types::dhcp_lease::DhcpLease;
use crate::overlay::types::dhcp_route::DhcpRoute;
use crate::overlay::types::overlay_plan::OverlayPlan;

#[test]
pub(in crate::overlay) fn dhcp_lease_round_trips_onto_the_interface() {
    let guest = port(
        "port-01a50318-a507-74fd-a86e-5acbda974701",
        "52:54:00:00:00:22",
        "10.0.0.1",
        "10.90.0.22",
    );
    let mut leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
    leg.dhcp = vec![DhcpLease {
        mac: "52:54:00:00:00:22".into(),
        address: "10.90.0.22".into(),
        dns: vec!["1.1.1.1".into()],
        routes: vec![DhcpRoute {
            destination: "10.8.0.9/32".into(),
            nexthop: "10.90.0.1".into(),
        }],
    }];
    let nets = [network(NET_A, 9, vec![leg], vec![guest])];
    let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    let dhcp = &plan.networks[0].routers[0].interfaces[0].dhcp;
    assert_eq!(dhcp.len(), 1);
    assert_eq!(dhcp[0].mac, "52:54:00:00:00:22");
    assert_eq!(dhcp[0].address, "10.90.0.22");
    assert_eq!(dhcp[0].dns, vec!["1.1.1.1".to_string()]);
    assert_eq!(dhcp[0].routes.len(), 1);
    assert_eq!(dhcp[0].routes[0].destination, "10.8.0.9/32");
    assert_eq!(dhcp[0].routes[0].nexthop, "10.90.0.1");
    let json = serde_json::to_string(&plan).unwrap();
    let back: OverlayPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(plan, back);
    assert!(json.contains("10.8.0.9/32"));
    assert!(json.contains("1.1.1.1"));
}
