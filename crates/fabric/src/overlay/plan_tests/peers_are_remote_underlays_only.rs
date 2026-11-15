// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::port;
use crate::overlay::tests::test_support::NET_A;

#[test]
pub(in crate::overlay) fn peers_are_remote_underlays_only() {
    let ports = vec![
        port(
            "port-01a50318-a507-74fd-a86e-5acbda974701",
            "52:54:00:00:00:22",
            "10.0.0.2",
            "10.90.0.22",
        ),
        port(
            "port-01a50318-a507-74fd-a86e-5acbda974702",
            "52:54:00:00:00:11",
            "10.0.0.1",
            "10.90.0.11",
        ),
    ];
    let nets = [network(NET_A, 1, vec![], ports)];
    let peers = vec![
        "10.0.0.10".to_string(),
        "10.0.0.2".to_string(),
        "10.0.0.2".to_string(),
    ];
    let plan = plan(&host("10.0.0.1"), &nets, &peers).unwrap();
    assert_eq!(
        plan.networks[0].peers,
        vec!["10.0.0.2".to_string(), "10.0.0.10".to_string()]
    );
    assert!(!plan.networks[0].peers.iter().any(|p| p == "0.0.0.0"));
    assert!(!plan.networks[0].peers.iter().any(|p| p == "10.0.0.1"));
    assert_eq!(plan.networks[0].ports[0].underlay, "10.0.0.2");
    assert_eq!(plan.networks[0].ports[0].mac, "52:54:00:00:00:22");
    assert_eq!(plan.networks[0].ports[1].underlay, "10.0.0.1");
    assert_eq!(plan.networks[0].ports[1].mac, "52:54:00:00:00:11");
    assert_eq!(plan.networks[0].local, "10.0.0.1");
}
