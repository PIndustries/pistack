// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::nic::ensure_ports;
use crate::overlay::{all_plans, host, placed, plan_for};
use crate::types::{Host, Machine};
use fabric::{Client, MockBackend, Network, NetworkSpec, Router, RouterSpec, SubnetSpec};
use pistack_store::Store;
use pistack_types::{kind, Id, Meta};
use std::sync::Arc;

#[tokio::test]
async fn ipv6_router_external_snat_is_a_private_ula() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
    let a = host("a", "10.1.0.1");
    store.collection::<Host>().create(a.clone()).unwrap();
    let project = Id::new(kind::PROJECT);
    let net = client
        .create_network(
            Meta::new(kind::NETWORK, "app6", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    store
        .collection::<Network>()
        .modify(&net.meta.id, |n| {
            n.status.segment_id = Some(11);
            Ok(())
        })
        .unwrap();
    let sub = client
        .create_subnet(
            Meta::new(kind::SUBNET, "app6-net", Some(project.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "fd00:90::/64".parse().unwrap(),
                gateway: Some("fd00:90::1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let machine = placed("box6", &project, "app6", &a);
    store
        .collection::<Machine>()
        .create(machine.clone())
        .unwrap();
    ensure_ports(&store, &machine).await.unwrap();
    let router = client
        .create_router(
            Meta::new(kind::ROUTER, "app6-edge", Some(project.clone())),
            RouterSpec {
                external_network: None,
                interfaces: vec![sub.meta.id],
                routes: vec![],
                vpc: None,
            },
        )
        .unwrap();
    store
        .collection::<Router>()
        .modify(&router.meta.id, |r| {
            r.status.external_ip = Some("fd00:66::9".parse().unwrap());
            Ok(())
        })
        .unwrap();

    let plan = plan_for(&all_plans(&store).unwrap(), "a");
    assert_eq!(plan.host, "10.1.0.1");
    let seg = plan
        .networks
        .iter()
        .find(|n| n.id == net.meta.id.to_string())
        .unwrap();
    assert_eq!(seg.routers.len(), 1);
    assert!(seg.routers[0].snat);
    assert_eq!(seg.routers[0].external_ip.as_deref(), Some("fd00:66::9"));
    assert_eq!(seg.routers[0].interfaces[0].cidr, "fd00:90::/64");
    assert_eq!(seg.routers[0].interfaces[0].gateway, "fd00:90::1");
    assert!(seg.routers[0].dnat.is_empty());

    store
        .collection::<Router>()
        .modify(&router.meta.id, |r| {
            r.status.external_ip = Some("2001:db8::9".parse().unwrap());
            Ok(())
        })
        .unwrap();
    let plan = plan_for(&all_plans(&store).unwrap(), "a");
    let seg = plan
        .networks
        .iter()
        .find(|n| n.id == net.meta.id.to_string())
        .unwrap();
    assert!(!seg.routers[0].snat);
    assert!(seg.routers[0].external_ip.is_none());
    assert_eq!(plan.host, "10.1.0.1");
}
