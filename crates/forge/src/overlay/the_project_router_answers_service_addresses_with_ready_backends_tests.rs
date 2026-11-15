// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{all_plans, host, plan_for, service, tenant, ServiceRecord};
use crate::types::Host;
use fabric::overlay::ServiceBackend;
use fabric::{Client, MockBackend};
use pistack_store::Store;
use pistack_types::{kind, Id};
use std::sync::Arc;

#[tokio::test]
async fn the_project_router_answers_service_addresses_with_ready_backends() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
    let a = host("a", "10.1.0.1");
    store.collection::<Host>().create(a.clone()).unwrap();
    let project = Id::new(kind::PROJECT);
    let other = Id::new(kind::PROJECT);
    let (net, port) = tenant(
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
    let before = plan_for(&all_plans(&store).unwrap(), "a");
    let fixed = before
        .networks
        .iter()
        .flat_map(|n| &n.ports)
        .find(|p| p.id == port)
        .unwrap()
        .address
        .clone();
    let services = store.collection::<ServiceRecord>();
    // One backend on the leg's subnet, one elsewhere.
    services
        .create(service(
            &project,
            "10.96.0.10",
            &[&fixed, "10.77.0.9"],
            &[
                ("TCP", 80, Some(8080)),
                ("UDP", 53, None),
                ("SCTP", 9, None),
            ],
        ))
        .unwrap();
    services
        .create(service(&project, "10.96.0.11", &[], &[("TCP", 443, None)]))
        .unwrap();
    services
        .create(service(
            &other,
            "10.96.0.12",
            &[&fixed],
            &[("TCP", 80, None)],
        ))
        .unwrap();

    let plans = all_plans(&store).unwrap();
    let plan = plan_for(&plans, "a");
    let seg = plan.networks.iter().find(|n| n.id == net).unwrap();
    let rules = &seg.routers[0].services;
    assert_eq!(rules.len(), 3, "{rules:?}");
    let web = rules.iter().find(|r| r.port == 80).unwrap();
    assert_eq!(web.address, "10.96.0.10");
    assert_eq!(web.protocol, "tcp");
    assert_eq!(
        web.backends,
        vec![ServiceBackend {
            address: fixed.clone(),
            port: 8080
        }]
    );
    let dns = rules.iter().find(|r| r.port == 53).unwrap();
    assert_eq!(dns.protocol, "udp");
    assert_eq!(dns.backends[0].port, 53);
    let idle = rules.iter().find(|r| r.port == 443).unwrap();
    assert!(idle.backends.is_empty(), "no ready backend is still listed");
    assert!(rules.iter().all(|r| r.address != "10.96.0.12"));
}
