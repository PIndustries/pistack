// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::Id;

use super::*;
use crate::types::{new_machine, MachineSpec};
use fabric::{NetworkSpec, SubnetSpec};
use pistack_types::{kind, Meta};

fn spec_net(name: &str) -> MachineSpec {
    MachineSpec {
        size: "s.1x1".into(),
        image: "ubuntu-24.04".into(),
        networks: vec![name.into()],
        ..MachineSpec::default()
    }
}

#[tokio::test]
async fn the_port_address_is_the_nic_and_a_router_marks_it_routed() {
    let store = Store::in_memory().unwrap();
    let client = fabric_client(&store);
    let project = Id::new(kind::PROJECT);
    let net = client
        .create_network(
            Meta::new(kind::NETWORK, "app", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    let sub = client
        .create_subnet(
            Meta::new(kind::SUBNET, "app-net", Some(project.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.90.0.0/24".parse().unwrap(),
                gateway: Some("10.90.0.1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let machine = new_machine("box", Some(project.clone()), spec_net("app"));
    ensure_ports(&store, &machine).await.unwrap();
    ensure_ports(&store, &machine).await.unwrap();
    assert_eq!(client.ports_of_device(&machine.meta.id).unwrap().len(), 1);

    let nics = attachments(&store, &machine).unwrap();
    assert_eq!(nics.len(), 1);
    assert_eq!(nics[0].address, "10.90.0.2");
    assert_ne!(nics[0].address, "10.90.0.1");
    assert_eq!(nics[0].prefix, 24);
    assert_eq!(nics[0].gateway, "10.90.0.1");
    assert!(!nics[0].routed);
    assert!(nics[0].mac.contains(':'));
    assert_eq!(nics[0].network_id, net.meta.id.to_string());

    let ext = NetworkSpec {
        external: true,
        ..Default::default()
    };
    let fleet = client
        .create_network(
            Meta::new(kind::NETWORK, "fleet", Some(project.clone())),
            ext,
        )
        .unwrap();
    client
        .create_subnet(
            Meta::new(kind::SUBNET, "fleet-pool", Some(project.clone())),
            SubnetSpec {
                network: fleet.meta.id.clone(),
                cidr: "10.66.66.80/28".parse().unwrap(),
                gateway: Some("10.66.66.81".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: false,
                host_routes: vec![],
            },
        )
        .unwrap();
    client
        .create_router(
            Meta::new(kind::ROUTER, "edge", Some(project)),
            fabric::RouterSpec {
                external_network: Some(fleet.meta.id),
                interfaces: vec![sub.meta.id],
                routes: vec![],
                vpc: None,
            },
        )
        .unwrap();
    let nics = attachments(&store, &machine).unwrap();
    assert!(nics[0].routed);

    let quiet = new_machine("demo", None, MachineSpec::default());
    ensure_ports(&store, &quiet).await.unwrap();
    assert!(attachments(&store, &quiet).unwrap().is_empty());
}

#[tokio::test]
async fn an_ovs_binding_is_copied_onto_the_nic() {
    let store = Store::in_memory().unwrap();
    let client = fabric_client(&store);
    let project = Id::new(kind::PROJECT);
    let net = client
        .create_network(
            Meta::new(kind::NETWORK, "app", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    client
        .create_subnet(
            Meta::new(kind::SUBNET, "app-net", Some(project.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.90.0.0/24".parse().unwrap(),
                gateway: Some("10.90.0.1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let machine = new_machine("box", Some(project), spec_net("app"));
    ensure_ports(&store, &machine).await.unwrap();
    let port = client
        .ports_of_device(&machine.meta.id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let iface = port.meta.id.to_string();
    client
        .ports()
        .modify(&port.meta.id, |p| {
            p.status.binding = Some(fabric::PortBinding {
                vif_type: "ovs".into(),
                vif_details: [
                    ("bridge".into(), "br-int".into()),
                    ("iface_id".into(), iface.clone()),
                ]
                .into_iter()
                .collect(),
            });
            Ok(())
        })
        .unwrap();
    let nics = attachments(&store, &machine).unwrap();
    let binding = nics[0].binding.as_ref().unwrap();
    assert_eq!(binding.vif_type, "ovs");
    assert_eq!(binding.vif_details["bridge"], "br-int");
    assert_eq!(binding.vif_details["iface_id"], iface);
}
