// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::nic::ensure_ports;
use crate::overlay::{all_plans, host, placed, plan_for, tenant};
use crate::types::{Host, Machine};
use fabric::{Client, MockBackend};
use pistack_store::Store;
use pistack_types::{kind, Id};
use std::sync::Arc;

#[tokio::test]
async fn two_projects_with_the_same_cidr_get_two_segments_and_the_peer_is_not_flooded() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
    let a = host("a", "10.1.0.1");
    let b = host("b", "10.1.0.2");
    store.collection::<Host>().create(a.clone()).unwrap();
    store.collection::<Host>().create(b.clone()).unwrap();

    let project = Id::new(kind::PROJECT);
    let other = Id::new(kind::PROJECT);
    let (net_a, port_a) = tenant(
        &client,
        &store,
        "app",
        &project,
        &a,
        "10.90.0.0/24",
        7,
        "10.66.66.84",
        "10.66.66.85",
    )
    .await;
    let (net_b, _) = tenant(
        &client,
        &store,
        "other",
        &other,
        &a,
        "10.90.0.0/24",
        8,
        "10.66.66.86",
        "10.66.66.87",
    )
    .await;

    // Second guest of the first project, on the other host.
    let machine_b = placed("box-b", &project, "app", &b);
    store
        .collection::<Machine>()
        .create(machine_b.clone())
        .unwrap();
    ensure_ports(&store, &machine_b).await.unwrap();

    let plans = all_plans(&store).unwrap();
    let plan_a = plan_for(&plans, "a");
    let plan_b = plan_for(&plans, "b");
    assert_eq!(plan_a.networks.len(), 2);
    assert_eq!(plan_b.networks.len(), 1);
    let seg_a = plan_a.networks.iter().find(|n| n.id == net_a).unwrap();
    let seg_other = plan_a.networks.iter().find(|n| n.id == net_b).unwrap();
    assert_ne!(seg_a.vni, seg_other.vni);
    assert_ne!(seg_a.bridge, seg_other.bridge);
    assert!(seg_a.bridge.starts_with("po"));
    assert!(seg_other.bridge.starts_with("po"));
    assert!(!seg_a.bridge.starts_with("pi"));
    assert_ne!(seg_a.routers[0].netns, seg_other.routers[0].netns);
    assert_eq!(seg_a.routers[0].interfaces[0].gateway, "10.90.0.1");
    assert_eq!(seg_other.routers[0].interfaces[0].gateway, "10.90.0.1");
    assert!(seg_a.metadata);
    assert!(!seg_a.peers.contains(&"0.0.0.0".to_string()));
    assert!(seg_a.peers.contains(&"10.1.0.2".to_string()));
    assert!(!seg_a.peers.contains(&"10.1.0.1".to_string()));
    assert!(seg_a
        .ports
        .iter()
        .any(|p| p.id == port_a && p.underlay == "10.1.0.1"));
    let remote = plan_b.networks[0].clone();
    assert!(
        remote.routers.is_empty(),
        "the router stays on the lowest underlay"
    );
    assert!(remote.metadata);
    assert!(remote.ports.iter().any(|p| p.underlay == "10.1.0.1"));
    assert!(remote.peers.contains(&"10.1.0.1".to_string()));
    assert!(seg_a.routers[0]
        .dnat
        .iter()
        .any(|d| d.floating == "10.66.66.85"));
}
