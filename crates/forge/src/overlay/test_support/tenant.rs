// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::nic::ensure_ports;
use crate::overlay::placed;
use crate::types::{Host, Machine};
use fabric::{
    Client, FloatingIp, FloatingIpSpec, FloatingIpStatus, Network, NetworkSpec, Router, RouterSpec,
    SubnetSpec,
};
use pistack_store::Store;
use pistack_types::{kind, Id, Meta};

#[allow(clippy::too_many_arguments)]
pub(in crate::overlay) async fn tenant(
    client: &Client,
    store: &Store,
    name: &str,
    project: &Id,
    host: &Host,
    cidr: &str,
    vni: u32,
    external_ip: &str,
    floating: &str,
) -> (String, String) {
    let net = client
        .create_network(
            Meta::new(kind::NETWORK, name, Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    store
        .collection::<Network>()
        .modify(&net.meta.id, |n| {
            n.status.segment_id = Some(vni);
            Ok(())
        })
        .unwrap();
    let sub = client
        .create_subnet(
            Meta::new(kind::SUBNET, format!("{name}-net"), Some(project.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: cidr.parse().unwrap(),
                gateway: Some("10.90.0.1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let machine = placed(name, project, name, host);
    store
        .collection::<Machine>()
        .create(machine.clone())
        .unwrap();
    ensure_ports(store, &machine).await.unwrap();
    let port = client
        .ports_of_device(&machine.meta.id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

    let ext_spec = NetworkSpec {
        external: true,
        ..Default::default()
    };
    let ext = client
        .create_network(
            Meta::new(kind::NETWORK, format!("{name}-ext"), Some(project.clone())),
            ext_spec,
        )
        .unwrap();
    client
        .create_subnet(
            Meta::new(kind::SUBNET, format!("{name}-pool"), Some(project.clone())),
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
            Meta::new(kind::ROUTER, format!("{name}-edge"), Some(project.clone())),
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
            r.status.external_ip = Some(external_ip.parse().unwrap());
            Ok(())
        })
        .unwrap();
    let fip_addr: std::net::IpAddr = floating.parse().unwrap();
    store
        .collection::<FloatingIp>()
        .create(FloatingIp {
            meta: Meta::new(
                kind::FLOATING_IP,
                format!("{name}-vip"),
                Some(project.clone()),
            ),
            spec: FloatingIpSpec {
                network: ext.meta.id,
                address: Some(fip_addr),
                port: Some(port.meta.id.clone()),
            },
            status: FloatingIpStatus {
                fixed_ip: Some(port.spec.fixed_ips[0]),
                router: Some(router.meta.id),
                ..FloatingIpStatus::default()
            },
        })
        .unwrap();
    (net.meta.id.to_string(), port.meta.id.to_string())
}
