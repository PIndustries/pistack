// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::nic::ensure_ports;
use crate::overlay::{all_plans, host, placed, plan_for};
use crate::types::{Host, Machine};
use fabric::{
    Client, Direction, EtherType, MockBackend, Network, NetworkSpec, Protocol, Remote, Rule,
    SecurityGroupSpec, SubnetSpec,
};
use pistack_store::Store;
use pistack_types::{kind, Id, Meta};
use std::sync::Arc;

#[tokio::test]
async fn group_members_expand_to_the_other_port_and_an_outsider_is_absent() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
    let a = host("a", "10.1.0.1");
    store.collection::<Host>().create(a.clone()).unwrap();
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
    let group = client
        .create_security_group(
            Meta::new(kind::SECGROUP, "web", Some(project.clone())),
            SecurityGroupSpec { rules: vec![] },
        )
        .unwrap();
    client
        .add_rule(
            &group.meta.id,
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(22, 22)
                .remote(Remote::Group(group.meta.id.clone())),
        )
        .unwrap();
    client
        .create_security_group(
            Meta::new(kind::SECGROUP, "other", Some(project.clone())),
            SecurityGroupSpec { rules: vec![] },
        )
        .unwrap();

    let mut members = Vec::new();
    for (name, sg) in [("one", "web"), ("two", "web"), ("out", "other")] {
        let mut machine = placed(name, &project, "app", &a);
        machine.spec.security_groups = vec![sg.into()];
        store
            .collection::<Machine>()
            .create(machine.clone())
            .unwrap();
        ensure_ports(&store, &machine).await.unwrap();
        let port = client
            .ports_of_device(&machine.meta.id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        members.push(port.meta.id.to_string());
    }

    let plan = plan_for(&all_plans(&store).unwrap(), "a");
    let seg = plan
        .networks
        .iter()
        .find(|n| n.id == net.meta.id.to_string())
        .unwrap();
    let port = |id: &str| seg.ports.iter().find(|p| p.id == id).unwrap();
    let one = port(&members[0]);
    let two = port(&members[1]);
    let outsider = port(&members[2]);
    let peer = format!("{}/32", two.address);
    let other = format!("{}/32", outsider.address);
    assert!(one.ingress.iter().any(|rule| rule.cidr == peer));
    assert!(two
        .ingress
        .iter()
        .any(|rule| rule.cidr == format!("{}/32", one.address)));
    assert!(one.ingress.iter().all(|rule| rule.cidr != other));
    assert!(two.ingress.iter().all(|rule| rule.cidr != other));
}
