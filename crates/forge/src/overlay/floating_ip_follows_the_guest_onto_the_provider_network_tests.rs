// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::nic::ensure_ports;
use crate::overlay::{all_plans, host, placed, plan_for, plans_for_machine};
use crate::types::{Host, Machine};
use fabric::{
    Client, FloatingIp, FloatingIpSpec, FloatingIpStatus, MockBackend, Network, NetworkKind,
    NetworkSpec, Router, RouterSpec, SubnetSpec,
};
use pistack_store::Store;
use pistack_types::{kind, Id, Meta};
use std::sync::Arc;

#[tokio::test]
async fn floating_ip_follows_the_guest_onto_the_provider_network() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
    let router_host = host("a", "10.1.0.1");
    let guest_host = host("b", "10.1.0.2");
    store
        .collection::<Host>()
        .create(router_host.clone())
        .unwrap();
    store
        .collection::<Host>()
        .create(guest_host.clone())
        .unwrap();
    let project = Id::new(kind::PROJECT);

    let net = client
        .create_network(
            Meta::new(kind::NETWORK, "app", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    store
        .collection::<Network>()
        .modify(&net.meta.id, |n| {
            n.status.segment_id = Some(7);
            Ok(())
        })
        .unwrap();
    let dns = "9.9.9.9";
    let sub = client
        .create_subnet(
            Meta::new(kind::SUBNET, "app-net", Some(project.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.90.0.0/24".parse().unwrap(),
                gateway: Some("10.90.0.1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![dns.parse().unwrap()],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();

    // A guest on the lower underlay keeps the router there. The floating
    // address belongs to the guest on the higher underlay.
    let anchor = placed("anchor", &project, "app", &router_host);
    store
        .collection::<Machine>()
        .create(anchor.clone())
        .unwrap();
    ensure_ports(&store, &anchor).await.unwrap();
    let guest = placed("guest", &project, "app", &guest_host);
    store.collection::<Machine>().create(guest.clone()).unwrap();
    ensure_ports(&store, &guest).await.unwrap();
    let guest_port = client
        .ports_of_device(&guest.meta.id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

    let mut ext_spec = NetworkSpec {
        external: true,
        ..Default::default()
    };
    ext_spec.kind = NetworkKind::Provider {
        physnet: "eno1".into(),
        vlan: Some(20),
    };
    let ext = client
        .create_network(
            Meta::new(kind::NETWORK, "edge", Some(project.clone())),
            ext_spec,
        )
        .unwrap();
    client
        .create_subnet(
            Meta::new(kind::SUBNET, "edge-pool", Some(project.clone())),
            SubnetSpec {
                network: ext.meta.id.clone(),
                cidr: "10.66.66.80/28".parse().unwrap(),
                gateway: Some("10.66.66.81".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: false,
                host_routes: vec![],
            },
        )
        .unwrap();
    let router = client
        .create_router(
            Meta::new(kind::ROUTER, "edge", Some(project.clone())),
            RouterSpec {
                external_network: Some(ext.meta.id.clone()),
                interfaces: vec![sub.meta.id],
                routes: vec![],
                vpc: None,
            },
        )
        .unwrap();
    store
        .collection::<Router>()
        .modify(&router.meta.id, |r| {
            r.status.external_ip = Some("10.66.66.84".parse().unwrap());
            Ok(())
        })
        .unwrap();
    let fip = store
        .collection::<FloatingIp>()
        .create(FloatingIp {
            meta: Meta::new(kind::FLOATING_IP, "guest-vip", Some(project.clone())),
            spec: FloatingIpSpec {
                network: ext.meta.id.clone(),
                address: Some("10.66.66.85".parse().unwrap()),
                port: Some(guest_port.meta.id.clone()),
            },
            status: FloatingIpStatus {
                fixed_ip: Some(guest_port.spec.fixed_ips[0]),
                router: Some(router.meta.id),
                ..FloatingIpStatus::default()
            },
        })
        .unwrap();

    let plans = all_plans(&store).unwrap();
    let on_guest = plan_for(&plans, "b");
    let on_router = plan_for(&plans, "a");
    let provider = on_guest
        .networks
        .iter()
        .find(|n| n.id == ext.meta.id.to_string())
        .unwrap();
    assert_eq!(provider.vni, 0);
    assert!(provider.routers.is_empty());
    assert!(provider
        .floating
        .iter()
        .any(|f| f.id == fip.meta.id.to_string()));
    assert!(provider
        .floating
        .iter()
        .all(|f| f.floating != "10.66.66.84"));
    let tenant = on_router
        .networks
        .iter()
        .find(|n| n.id == net.meta.id.to_string())
        .unwrap();
    assert!(tenant.routers[0]
        .dnat
        .iter()
        .all(|d| d.floating != "10.66.66.85"));
    let lease = tenant.routers[0].interfaces[0]
        .dhcp
        .iter()
        .find(|lease| lease.mac == guest_port.spec.mac)
        .unwrap();
    assert!(lease.dns.iter().any(|server| server == dns));

    let scoped = plans_for_machine(&store, &guest).unwrap();
    let scoped_guest = plan_for(&scoped, "b");
    assert!(
        scoped_guest
            .networks
            .iter()
            .any(|n| n.id == ext.meta.id.to_string()),
        "the external segment stays even though the machine does not name it"
    );
}
